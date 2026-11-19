//! The pin of http-client.default-transport-h2: the protocol and `User-Agent` an https request
//! of each client arrives with, next to Go's `http.DefaultTransport` (the currency fetch and the
//! analytics client) and router.go's general Transport (`testdata/golden/default_client.json`,
//! `testdata/go/defaultclient.go`).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use pbs_http_client::{Builder, HttpClientSettings, ProxyEnv};
    use serde_json::Value;
    use tokio::sync::mpsc;

    use crate::support::{
        BoxError, RawServer, TestPki, TestResult, check_eq, fetch, golden, list, request, run, text,
    };

    /// What one case shows: the client, whether the server offered h2, the response's protocol
    /// and the User-Agent the server read.
    type Outcome = (String, bool, String, String);

    /// Sends one https GET with `settings` to a server offering `alpn`.
    async fn arrival(
        settings: HttpClientSettings,
        pki: &TestPki,
        alpn: &[&[u8]],
    ) -> Result<(String, String), BoxError> {
        let (sender, mut agents) = mpsc::unbounded_channel();
        let server = RawServer::start_tls(pki.acceptor(alpn)?, move |mut conn| {
            let sender = sender.clone();
            async move {
                if let Some(request) = conn.read_request().await {
                    let agent = request
                        .head
                        .iter()
                        .filter_map(|line| line.split_once(": "))
                        .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
                        .map_or_else(String::new, |(_, value)| value.to_owned());
                    let _sent = sender.send(agent);
                    conn.write(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                }
            }
        })
        .await?;
        let client = Builder::new(settings, ProxyEnv::default())
            .roots(pki.roots())
            .loopback_only(0)
            .build()?;
        let url = format!("https://127.0.0.1:{}/d", server.port());
        let fetched = fetch(client.send(request("GET", &url)?, None, None).await?).await;
        let proto = format!("{:?}", fetched.version);
        drop(fetched.body?);
        let agent = agents.recv().await.ok_or("the server read no request")?;
        drop(client);
        let _counts = server.close().await;
        Ok((proto, agent))
    }

    // Go: net/http/transport.go DefaultTransport (ForceAttemptHTTP2) and
    // Transport.onceSetNextProtoDefaults; net/http/h2_bundle.go (the HTTP/2 User-Agent).
    #[test]
    fn plain_client_stays_on_http1() -> TestResult {
        run(async {
            let doc = golden("default_client.json")?;
            let pki = TestPki::new()?;
            let mut differ: Vec<(Outcome, Outcome)> = Vec::new();
            for case in list(&doc, "/cases")? {
                let client = text(case, "/client")?.to_owned();
                let server_h2 = case
                    .get("server_h2")
                    .and_then(Value::as_bool)
                    .ok_or("no server_h2")?;
                let settings = if client == "default" {
                    HttpClientSettings::DEFAULT_TRANSPORT
                } else {
                    HttpClientSettings::GENERAL
                };
                let alpn: &[&[u8]] = if server_h2 {
                    &[b"h2", b"http/1.1"]
                } else {
                    &[b"http/1.1"]
                };
                let (proto, agent) = arrival(settings, &pki, alpn).await?;
                let go = (
                    client.clone(),
                    server_h2,
                    text(case, "/proto")?.to_owned(),
                    text(case, "/user_agent")?.to_owned(),
                );
                let rust = (client, server_h2, proto, agent);
                if rust != go {
                    differ.push((go, rust));
                }
            }
            let outcome = |proto: &str, agent: &str| -> Outcome {
                (
                    "default".to_owned(),
                    true,
                    proto.to_owned(),
                    agent.to_owned(),
                )
            };
            check_eq(
                &differ,
                &vec![(
                    outcome("HTTP/2.0", "Go-http-client/2.0"),
                    outcome("HTTP/1.1", "Go-http-client/1.1"),
                )],
                "the cases that differ (Go, Rust)",
            )
        })
    }
}
