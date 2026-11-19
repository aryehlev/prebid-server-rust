//! Pins of `reqwest` against Go's `net/http` client as `router/router.go` builds it: the request
//! head each one writes for a bare GET and for a bidder's JSON POST
//! (`testdata/golden/http_client.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{BoxError, TestResult, check_eq, check_table, golden, list, run, text};
    use reqwest::{Client, RequestBuilder, header};
    use serde_json::Value;
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
        net::TcpListener,
    };

    /// Serves one connection on a loopback listener and returns the head of the request `send`
    /// makes to the URL it is given, as `testdata/go/httpclient.go` captures it: the request
    /// line and the header lines as they came over the wire, the port replaced by `PORT`.
    async fn capture<F>(send: F) -> Result<Vec<String>, BoxError>
    where
        F: FnOnce(String) -> RequestBuilder,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let mut reader = BufReader::new(stream);
            let mut lines = Vec::new();
            let mut length = 0_u64;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await? == 0 {
                    return Err::<Vec<String>, BoxError>("the request head ended early".into());
                }
                let line = line.trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse()?;
                }
                lines.push(line.replace(&format!(":{port}"), ":PORT"));
            }
            let mut body = Vec::new();
            let read = (&mut reader).take(length).read_to_end(&mut body).await?;
            check_eq(&u64::try_from(read)?, &length, "the body")?;
            reader
                .into_inner()
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await?;
            Ok(lines)
        });
        let response = send(format!("http://127.0.0.1:{port}/headers"))
            .send()
            .await?;
        drop(response.bytes().await?);
        server.await?
    }

    /// The client the pins send with: `reqwest`'s defaults with the workspace features, and no
    /// proxy (Go's `ProxyFromEnvironment` never proxies loopback either).
    fn client() -> Result<Client, BoxError> {
        Ok(Client::builder().no_proxy().build()?)
    }

    /// The heads `reqwest` writes for the golden's two requests: a GET with no header set and a
    /// POST with the body, `Content-Type` and `Accept` most adapters set.
    async fn rust_heads(post_body: String) -> Result<(Vec<String>, Vec<String>), BoxError> {
        let get_client = client()?;
        let get = capture(|url| get_client.get(url)).await?;
        let post_client = client()?;
        let post = capture(|url| {
            post_client
                .post(url)
                .header(header::CONTENT_TYPE, "application/json;charset=utf-8")
                .header(header::ACCEPT, "application/json")
                .body(post_body)
        })
        .await?;
        Ok((get, post))
    }

    /// The golden's head lines at `pointer`.
    fn go_head(doc: &Value, pointer: &str) -> Result<Vec<String>, BoxError> {
        list(doc, pointer)?
            .iter()
            .map(|line| {
                line.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{pointer} has a line that is not a string").into())
            })
            .collect()
    }

    /// The line of `head` for the header `name` (any case), or an empty string.
    fn header_line(head: &[String], name: &str) -> String {
        head.iter()
            .find(|line| {
                line.split_once(':')
                    .is_some_and(|(found, _)| found.eq_ignore_ascii_case(name))
            })
            .cloned()
            .unwrap_or_default()
    }

    /// Go's `net/http` sends `User-Agent: Go-http-client/1.1` when the caller set none;
    /// `reqwest` sends no `User-Agent` at all. The whole heads are pinned too: they also show the
    /// header case and order hyper writes (the HTTP client's `header-case` and `header-order`
    /// entries) and `reqwest`'s default `Accept` (`go_difference_default_accept_header`).
    // Go: router/router.go:143-174 (the bidder and cache clients' Transports) and net/http's
    // Request.write (defaultUserAgent); adapters/appnexus/appnexus.go:321-322 (the headers an
    // adapter sets)
    #[test]
    fn go_difference_no_default_user_agent() -> TestResult {
        let doc = golden("http_client.json")?;
        let (get, post) = run(rust_heads(text(&doc, "/post_body")?.to_owned()))?;
        let (go_get, go_post) = (go_head(&doc, "/get")?, go_head(&doc, "/post")?);
        check_table(
            &[
                vec![
                    "GET".to_owned(),
                    header_line(&go_get, "user-agent"),
                    header_line(&get, "user-agent"),
                ],
                vec![
                    "POST".to_owned(),
                    header_line(&go_post, "user-agent"),
                    header_line(&post, "user-agent"),
                ],
            ],
            &[
                &["GET", "User-Agent: Go-http-client/1.1", ""],
                &["POST", "User-Agent: Go-http-client/1.1", ""],
            ],
            "request, net/http's User-Agent, reqwest's",
        )?;
        check_table(
            &[go_get, get, go_post, post],
            &[
                &[
                    "GET /headers HTTP/1.1",
                    "Host: 127.0.0.1:PORT",
                    "User-Agent: Go-http-client/1.1",
                    "Accept-Encoding: gzip",
                ],
                &[
                    "GET /headers HTTP/1.1",
                    "accept: */*",
                    "accept-encoding: gzip",
                    "host: 127.0.0.1:PORT",
                ],
                &[
                    "POST /headers HTTP/1.1",
                    "Host: 127.0.0.1:PORT",
                    "User-Agent: Go-http-client/1.1",
                    "Content-Length: 10",
                    "Accept: application/json",
                    "Content-Type: application/json;charset=utf-8",
                    "Accept-Encoding: gzip",
                ],
                &[
                    "POST /headers HTTP/1.1",
                    "content-type: application/json;charset=utf-8",
                    "accept: application/json",
                    "accept-encoding: gzip",
                    "host: 127.0.0.1:PORT",
                    "content-length: 10",
                ],
            ],
            "the heads: net/http GET, reqwest GET, net/http POST, reqwest POST",
        )
    }

    /// `reqwest`'s `ClientBuilder::new` puts `Accept: */*` among the client's default headers,
    /// so a request whose caller set no `Accept` (the currency and vendor-list GETs, and
    /// adapters that set none) carries one; `net/http` sends none. A caller's `Accept` replaces
    /// the default, and `ClientBuilder::default_headers` can only add to the defaults, not drop
    /// one.
    // Go: currency/rate_converter.go:53 (a GET with no header set) through
    // router/router.go:143-174, and net/http's Request.write, which adds no Accept
    #[test]
    fn go_difference_default_accept_header() -> TestResult {
        let doc = golden("http_client.json")?;
        let (get, post) = run(rust_heads(text(&doc, "/post_body")?.to_owned()))?;
        check_table(
            &[
                vec![
                    "GET".to_owned(),
                    header_line(&go_head(&doc, "/get")?, "accept"),
                    header_line(&get, "accept"),
                ],
                vec![
                    "POST".to_owned(),
                    header_line(&go_head(&doc, "/post")?, "accept"),
                    header_line(&post, "accept"),
                ],
            ],
            &[
                &["GET", "", "accept: */*"],
                &[
                    "POST",
                    "Accept: application/json",
                    "accept: application/json",
                ],
            ],
            "request, net/http's Accept, reqwest's",
        )
    }
}
