//! Pins of `axum` with `tower-http`'s `CorsLayer` against `rs/cors` as `router.SupportCORS`
//! configures it: the answers to CORS preflights (`testdata/golden/server.json`).

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        http::{HeaderName, Method, StatusCode, header},
        routing::post,
    };
    use pbs_differences::{BoxError, TestResult, check_table, golden, list, number, run, text};
    use reqwest::Client;
    use serde_json::Value;
    use tokio::{net::TcpListener, sync::oneshot};
    use tower_http::cors::{AllowOrigin, CorsLayer};

    /// `router.SupportCORS`'s options on `tower-http`: every origin reflected, with credentials,
    /// `rs/cors`' default methods (GET, POST and HEAD) and the four allowed request headers.
    fn cors() -> CorsLayer {
        CorsLayer::new()
            .allow_origin(AllowOrigin::mirror_request())
            .allow_credentials(true)
            .allow_methods([Method::GET, Method::POST, Method::HEAD])
            .allow_headers([
                header::ORIGIN,
                HeaderName::from_static("x-requested-with"),
                header::CONTENT_TYPE,
                header::ACCEPT,
            ])
    }

    /// The CORS headers and `Vary` of one answer as `name: value` lines, names in lower case
    /// (header case is the server's `header-case` entry), sorted by name.
    fn cors_lines<'h, I: Iterator<Item = (String, &'h str)>>(headers: I) -> String {
        let mut lines: Vec<String> = headers
            .filter(|(name, _)| name.starts_with("access-control-") || name == "vary")
            .map(|(name, value)| format!("{name}: {value}"))
            .collect();
        lines.sort();
        lines.join("\n")
    }

    /// What `rs/cors` answered to each golden preflight: the requested headers, the status and
    /// the CORS headers.
    fn go_answers(doc: &Value) -> Result<Vec<Vec<String>>, BoxError> {
        list(doc, "/preflights")?
            .iter()
            .map(|preflight| {
                let headers = preflight
                    .get("headers")
                    .and_then(Value::as_object)
                    .ok_or("a preflight without headers")?;
                let pairs = headers.iter().flat_map(|(name, values)| {
                    values
                        .as_array()
                        .map_or_default(Vec::as_slice)
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|value| (name.to_ascii_lowercase(), value))
                });
                Ok(vec![
                    text(preflight, "/request_headers")?.to_owned(),
                    number(preflight, "/status")?.to_string(),
                    cors_lines(pairs),
                ])
            })
            .collect()
    }

    /// Sends the golden's preflights to the route behind [`cors`] on a loopback server.
    async fn rust_answers(doc: &Value) -> Result<Vec<Vec<String>>, BoxError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let authority = listener.local_addr()?;
        let path = text(doc, "/path")?;
        let app = Router::new()
            .route(path, post(|| async { StatusCode::OK }))
            .layer(cors());
        let (stop, stopped) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    stopped.await.unwrap_or_default();
                })
                .await
        });
        let client = Client::builder().no_proxy().build()?;
        let mut rows = Vec::new();
        for preflight in list(doc, "/preflights")? {
            let requested = text(preflight, "/request_headers")?;
            let response = client
                .request(Method::OPTIONS, format!("http://{authority}{path}"))
                .header(header::ORIGIN, text(doc, "/origin")?)
                .header(
                    header::ACCESS_CONTROL_REQUEST_METHOD,
                    text(doc, "/request_method")?,
                )
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, requested)
                .send()
                .await?;
            let pairs = response.headers().iter().filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value))
            });
            rows.push(vec![
                requested.to_owned(),
                response.status().as_u16().to_string(),
                cors_lines(pairs),
            ]);
        }
        stop.send(()).map_err(|()| "the server stopped early")?;
        server.await??;
        Ok(rows)
    }

    /// `rs/cors` answers a preflight with 204, echoes the requested method and only the
    /// requested headers it allows, and answers a preflight that asks for a header it does not
    /// allow with no CORS headers at all. `tower-http` answers 200 with the configured method
    /// and header lists every time.
    // Go: router/router.go:377-385 (SupportCORS on rs/cors v1.11.0), wrapped around the whole
    // router at main.go:82
    #[test]
    fn go_difference_preflight_status_and_headers() -> TestResult {
        let doc = golden("server.json")?;
        let mut rows = go_answers(&doc)?;
        rows.extend(run(rust_answers(&doc))?);
        let go_allowed = |headers: &str| {
            format!(
                "access-control-allow-credentials: true\naccess-control-allow-headers: \
                 {headers}\naccess-control-allow-methods: POST\naccess-control-allow-origin: \
                 https://publisher.example\nvary: Origin, Access-Control-Request-Method, \
                 Access-Control-Request-Headers"
            )
        };
        let tower = "access-control-allow-credentials: true\naccess-control-allow-headers: \
                     origin,x-requested-with,content-type,accept\naccess-control-allow-methods: \
                     GET,POST,HEAD\naccess-control-allow-origin: https://publisher.example\nvary: \
                     origin, access-control-request-method, access-control-request-headers";
        let go_one = go_allowed("content-type");
        let go_two = go_allowed("content-type,x-requested-with");
        check_table(
            &rows,
            &[
                &["content-type", "204", &go_one],
                &["content-type,x-requested-with", "204", &go_two],
                &[
                    "x-custom",
                    "204",
                    "vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
                ],
                &["content-type", "200", tower],
                &["content-type,x-requested-with", "200", tower],
                &["x-custom", "200", tower],
            ],
            "requested headers, status, CORS headers: rs/cors first, then tower-http",
        )
    }
}
