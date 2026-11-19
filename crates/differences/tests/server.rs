//! Pins of `axum` with `tower-http`'s `CorsLayer` against `rs/cors` as `router.SupportCORS`
//! configures it: the answers to CORS preflights and to the other requests browsers and servers
//! send (`testdata/golden/server.json`).

#[cfg(test)]
mod tests {
    use std::{io, net::SocketAddr};

    use axum::{
        Router,
        http::{HeaderName, Method, StatusCode, header},
        routing::any,
    };
    use pbs_differences::{
        BoxError, TestResult, check_eq, check_table, golden, list, number, run, text,
    };
    use reqwest::{Client, Response};
    use serde_json::Value;
    use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
    use tower_http::cors::{AllowOrigin, CorsLayer};

    /// The header the handler behind the CORS layer sets on every answer it writes, as the
    /// golden's handler does, so an answer shows whether the request reached the handler.
    const HANDLER_HEADER: &str = "x-handler";

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

    /// `called` when the answer carries [`HANDLER_HEADER`], else `not called`.
    fn handler(headers: &[(String, &str)]) -> String {
        if headers.iter().any(|(name, _)| name == HANDLER_HEADER) {
            "called"
        } else {
            "not called"
        }
        .to_owned()
    }

    /// The headers of one golden answer, names in lower case.
    fn go_headers(answer: &Value) -> Result<Vec<(String, &str)>, BoxError> {
        let headers = answer
            .get("headers")
            .and_then(Value::as_object)
            .ok_or("an answer without headers")?;
        Ok(headers
            .iter()
            .flat_map(|(name, values)| {
                values
                    .as_array()
                    .map_or_default(Vec::as_slice)
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|value| (name.to_ascii_lowercase(), value))
            })
            .collect())
    }

    /// The headers of one answer of the loopback server (names in lower case, as hyper keeps
    /// them).
    fn rust_headers(response: &Response) -> Vec<(String, &str)> {
        response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value))
            })
            .collect()
    }

    /// What `rs/cors` answered to each golden preflight: the requested headers, the status and
    /// the CORS headers.
    fn go_answers(doc: &Value) -> Result<Vec<Vec<String>>, BoxError> {
        list(doc, "/preflights")?
            .iter()
            .map(|preflight| {
                Ok(vec![
                    text(preflight, "/request_headers")?.to_owned(),
                    number(preflight, "/status")?.to_string(),
                    cors_lines(go_headers(preflight)?.into_iter()),
                ])
            })
            .collect()
    }

    /// What `rs/cors` answered to each golden request that is not a preflight: the method, the
    /// Origin (empty for none), the status, whether the handler wrote the answer and the CORS
    /// headers.
    fn go_actual(doc: &Value) -> Result<Vec<Vec<String>>, BoxError> {
        list(doc, "/actual_requests")?
            .iter()
            .map(|request| {
                let headers = go_headers(request)?;
                Ok(vec![
                    text(request, "/method")?.to_owned(),
                    text(request, "/origin")?.to_owned(),
                    number(request, "/status")?.to_string(),
                    handler(&headers),
                    cors_lines(headers.into_iter()),
                ])
            })
            .collect()
    }

    /// A loopback server with one route behind [`cors`], whose handler answers every method with
    /// 200 and [`HANDLER_HEADER`].
    struct Server {
        authority: SocketAddr,
        shutdown: oneshot::Sender<()>,
        task: JoinHandle<io::Result<()>>,
    }

    impl Server {
        /// Serves `path` on a fresh loopback port.
        async fn start(path: &str) -> Result<Self, BoxError> {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let authority = listener.local_addr()?;
            let app = Router::new()
                .route(
                    path,
                    any(|| async { ([(HANDLER_HEADER, "called")], StatusCode::OK) }),
                )
                .layer(cors());
            let (shutdown, stopped) = oneshot::channel::<()>();
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        stopped.await.unwrap_or_default();
                    })
                    .await
            });
            Ok(Self {
                authority,
                shutdown,
                task,
            })
        }

        /// Stops the server and waits for it.
        async fn stop(self) -> TestResult {
            self.shutdown
                .send(())
                .map_err(|()| "the server stopped early")?;
            self.task.await??;
            Ok(())
        }
    }

    /// Sends the golden's preflights to the loopback server.
    async fn rust_answers(doc: &Value) -> Result<Vec<Vec<String>>, BoxError> {
        let path = text(doc, "/path")?;
        let server = Server::start(path).await?;
        let client = Client::builder().no_proxy().build()?;
        let mut rows = Vec::new();
        for preflight in list(doc, "/preflights")? {
            let requested = text(preflight, "/request_headers")?;
            let response = client
                .request(
                    Method::OPTIONS,
                    format!("http://{}{path}", server.authority),
                )
                .header(header::ORIGIN, text(doc, "/origin")?)
                .header(
                    header::ACCESS_CONTROL_REQUEST_METHOD,
                    text(doc, "/request_method")?,
                )
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, requested)
                .send()
                .await?;
            rows.push(vec![
                requested.to_owned(),
                response.status().as_u16().to_string(),
                cors_lines(rust_headers(&response).into_iter()),
            ]);
        }
        server.stop().await?;
        Ok(rows)
    }

    /// Sends the golden's other requests to the loopback server, each with the golden's Origin
    /// or none.
    async fn rust_actual(doc: &Value) -> Result<Vec<Vec<String>>, BoxError> {
        let path = text(doc, "/path")?;
        let server = Server::start(path).await?;
        let client = Client::builder().no_proxy().build()?;
        let mut rows = Vec::new();
        for request in list(doc, "/actual_requests")? {
            let method = text(request, "/method")?;
            let origin = text(request, "/origin")?;
            let mut builder = client.request(
                Method::from_bytes(method.as_bytes())?,
                format!("http://{}{path}", server.authority),
            );
            if !origin.is_empty() {
                builder = builder.header(header::ORIGIN, origin);
            }
            let response = builder.send().await?;
            let headers = rust_headers(&response);
            rows.push(vec![
                method.to_owned(),
                origin.to_owned(),
                response.status().as_u16().to_string(),
                handler(&headers),
                cors_lines(headers.into_iter()),
            ]);
        }
        server.stop().await?;
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

    /// On a request that is not a preflight, `rs/cors` writes `Vary: Origin` and, only when the
    /// request has an Origin and a method it allows (GET, POST and HEAD, and OPTIONS always),
    /// the allowed origin and credentials; an OPTIONS without `Access-Control-Request-Method`
    /// goes on to the handler. `tower-http` writes `Access-Control-Allow-Credentials` on every
    /// answer, the allowed origin whatever the method, and the three preflight names in `Vary`,
    /// and it answers every OPTIONS itself as a preflight, so that request never reaches the
    /// handler.
    // Go: router/router.go:377-385 (SupportCORS on rs/cors v1.11.0), wrapped around the whole
    // router at main.go:82; rs/cors v1.11.0's cors.go:274 (only an OPTIONS with
    // Access-Control-Request-Method is a preflight), :400-446 (handleActualRequest) and :487-502
    // (isMethodAllowed)
    #[test]
    fn go_difference_actual_request_headers() -> TestResult {
        let doc = golden("server.json")?;
        check_eq(
            text(&doc, "/handler_header")?.to_ascii_lowercase().as_str(),
            HANDLER_HEADER,
            "the header the handler sets",
        )?;
        let mut rows = go_actual(&doc)?;
        rows.extend(run(rust_actual(&doc))?);
        let origin = "https://publisher.example";
        let go_allowed = "access-control-allow-credentials: true\naccess-control-allow-origin: \
                          https://publisher.example\nvary: Origin";
        let tower_vary = "vary: origin, access-control-request-method, \
                          access-control-request-headers";
        let tower_allowed = format!(
            "access-control-allow-credentials: true\naccess-control-allow-origin: \
             https://publisher.example\n{tower_vary}"
        );
        let tower_no_origin = format!("access-control-allow-credentials: true\n{tower_vary}");
        let tower_preflight = format!(
            "access-control-allow-credentials: true\naccess-control-allow-headers: \
             origin,x-requested-with,content-type,accept\naccess-control-allow-methods: \
             GET,POST,HEAD\naccess-control-allow-origin: https://publisher.example\n{tower_vary}"
        );
        check_table(
            &rows,
            &[
                &["POST", origin, "200", "called", go_allowed],
                &["GET", origin, "200", "called", go_allowed],
                &["PUT", origin, "200", "called", "vary: Origin"],
                &["POST", "", "200", "called", "vary: Origin"],
                &["OPTIONS", origin, "200", "called", go_allowed],
                &["POST", origin, "200", "called", &tower_allowed],
                &["GET", origin, "200", "called", &tower_allowed],
                &["PUT", origin, "200", "called", &tower_allowed],
                &["POST", "", "200", "called", &tower_no_origin],
                &["OPTIONS", origin, "200", "not called", &tower_preflight],
            ],
            "method, Origin, status, handler, CORS headers: rs/cors first, then tower-http",
        )
    }
}
