//! Pins of the currency fetch against Go's (`testdata/golden/fetch.golden`, written by
//! `testdata/go/fetch.go`): `RateConverter.Run` on main.go's `&http.Client{}`, against loopback
//! servers that answer with the bytes Go's did. The outcomes, the fallback to the constant rates,
//! the requests, Prebid's error texts and log lines and the deadline's texts are Go's; the words
//! of the client, the body reader and the URL parser are reqwest's and the url crate's where Go
//! has net/http's and net/url's (`currency.fetch-error-texts`), and the JSON decoder's
//! `serde_json`'s where Go has jsoniter's (`json.error-texts`). The url crate reads some URLs
//! otherwise than net/url (`currency.fetch-url`), and the credentials of a URL go out otherwise
//! than net/http sends them (`currency.fetch-credentials`).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use crate::support::{Replayed, Run, TestResult, check, check_eq, fetch_golden, replay};

    /// A text of the Rust fetch whose words are not all Go's: it matches `rust`, in which `*`
    /// stands for the library's own words, and Go's text matches `go`, its `*` standing for
    /// net/http's or net/url's, and the two differ.
    #[derive(Debug, Clone, Copy)]
    struct Library {
        rust: &'static str,
        go: &'static str,
    }

    /// The library's words where Go's text has its own, around Go's words.
    const fn library(pattern: &'static str) -> Library {
        Library {
            rust: pattern,
            go: pattern,
        }
    }

    /// The scenarios whose texts are not all Go's: the words of the calls' error and those of the
    /// drain line after `error draining conversion rates response body: `. Every other text is
    /// Go's, and an update line is Go's words followed by the call's error.
    fn library_words(scenario: &str) -> (Option<Library>, Option<Library>) {
        const SENT: &str = "Get \"http://127.0.0.1:PORT/latest.json\": *";
        const BODY: &str = "the currency rates request failed: *";
        const JSON: &str = "the currency rates request failed to parse json: *";
        match scenario {
            "empty_body" | "invalid_json" => (Some(library(JSON)), None),
            "gzip_corrupt" | "body_cut_short" => (Some(library(BODY)), Some(library("*"))),
            "body_cut_short_500" => (None, Some(library("*"))),
            // Go's drain reads the closed connection again; the body here keeps its deadline.
            "timeout_reading_body" => (
                None,
                Some(Library {
                    rust: "context deadline exceeded",
                    go: "read tcp 127.0.0.1:PORT->127.0.0.1:PORT: use of closed network connection",
                }),
            ),
            "server_closes" | "malformed_response" | "connection_refused" => {
                (Some(library(SENT)), None)
            }
            // url.Parse takes a URL without a scheme, which client.Do then refuses; the url
            // crate refuses it at once.
            "relative_url" => (
                Some(Library {
                    rust: "parse \"invalidurl\": *",
                    go: "Get \"invalidurl\": *",
                }),
                None,
            ),
            "unparsable_url" => (Some(library("parse \"http://[::1\": *")), None),
            // `{:?}` quotes a control character otherwise than `%q`.
            "url_control_character" => (
                Some(Library {
                    rust: "parse \"http://[::1\\u{1}]/latest.json\": *",
                    go: "parse \"http://[::1\\x01]/latest.json\": *",
                }),
                None,
            ),
            // Both show the password as `***`.
            "url_credentials_refused" => (
                Some(library(
                    "Get \"http://user:***@127.0.0.1:PORT/latest.json\": *",
                )),
                None,
            ),
            "unsupported_scheme" => (
                Some(library("Get \"ftp://127.0.0.1/latest.json\": *")),
                None,
            ),
            _ => (None, None),
        }
    }

    /// What the last `*` of `pattern` stands for in `text`, at least one byte; empty for a
    /// pattern without one that is `text`. A `*` before it is itself (the password Go and the
    /// port show as `***`).
    fn wildcard<'t>(pattern: &str, text: &'t str) -> Option<&'t str> {
        match pattern.rsplit_once('*') {
            None => (pattern == text).then_some(""),
            Some((prefix, suffix)) => text
                .strip_prefix(prefix)?
                .strip_suffix(suffix)
                .filter(|words| !words.is_empty()),
        }
    }

    /// Checks a text of the Rust fetch against Go's: Go's own, or as `library` says.
    fn check_text(rust: &str, go: &str, library: Option<Library>, what: &str) -> TestResult {
        let Some(library) = library else {
            return check_eq(rust, go, what);
        };
        check(
            wildcard(library.rust, rust).is_some(),
            &format!("{what}: {rust:?} is not {:?}", library.rust),
        )?;
        check(
            wildcard(library.go, go).is_some(),
            &format!("{what}: Go's {go:?} is not {:?}", library.go),
        )?;
        check(rust != go, &format!("{what}: {rust:?} is Go's text"))
    }

    /// The drain line's words before its error (`currency/rate_converter.go:66`).
    const DRAIN: &str = "error draining conversion rates response body: ";

    /// The update lines' words before the error (`currency/rate_converter.go:97-99`).
    const UPDATE: [&str; 2] = [
        "Error updating conversion rates, falling back to constant rates: ",
        "Error updating conversion rates: ",
    ];

    /// Checks a line the Rust fetch logged against Go's: the drain line with its words as
    /// `drain` says, or an update line, Go's words before the call's error.
    fn check_line(
        (rust, go): (&str, &str),
        (rust_error, go_error): (Option<&str>, Option<&str>),
        drain: Option<Library>,
        what: &str,
    ) -> TestResult {
        if let Some(go_words) = go.strip_prefix(DRAIN) {
            let rust_words = rust
                .strip_prefix(DRAIN)
                .ok_or_else(|| format!("{what}: {rust:?} is not the drain line"))?;
            return check_text(rust_words, go_words, drain, what);
        }
        for words in UPDATE {
            if let Some(go_rest) = go.strip_prefix(words) {
                check_eq(&Some(go_rest), &go_error, what)?;
                return check_eq(&rust.strip_prefix(words), &rust_error, what);
            }
        }
        Err(format!("{what}: Go logged {go:?}, no line of fetch's or update's").into())
    }

    #[test]
    fn error_texts() -> TestResult {
        // Go: currency/rate_converter.go:49-102 (the errors of fetch and the lines fetch and
        // update log)
        let scenarios = fetch_golden()?;
        let replayed = replay(&scenarios)?;
        check_eq(&replayed.len(), &scenarios.len(), "scenarios")?;
        for (scenario, rust) in scenarios.iter().zip(&replayed) {
            // Go's fetch refuses these URLs, or sends them otherwise: fetch_urls_differ.
            if url_difference(&scenario.name).is_some() {
                continue;
            }
            let (error_words, drain_words) = library_words(&scenario.name);
            check_eq(&rust.runs.len(), &scenario.runs.len(), &scenario.name)?;
            for (n, (go_run, rust_run)) in scenario.runs.iter().zip(&rust.runs).enumerate() {
                let what = format!("{} call {n}", scenario.name);
                match (&rust_run.error, &go_run.error) {
                    (None, None) => {}
                    (Some(rust_error), Some(go_error)) => {
                        check_text(rust_error, go_error, error_words, &what)?;
                    }
                    (rust_error, go_error) => {
                        return Err(format!("{what}: {rust_error:?}, Go's {go_error:?}").into());
                    }
                }
                check_eq(
                    &rust_run.logs.len(),
                    &go_run.logs.len(),
                    &format!("{what}: lines {:?}, Go's {:?}", rust_run.logs, go_run.logs),
                )?;
                for ((rust_severity, rust_line), (go_severity, go_line)) in
                    rust_run.logs.iter().zip(&go_run.logs)
                {
                    check_eq(rust_severity, go_severity, &what)?;
                    check_line(
                        (rust_line, go_line),
                        (rust_run.error.as_deref(), go_run.error.as_deref()),
                        drain_words,
                        &what,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// A request head's request line and its header fields, names in lower case, sorted.
    fn head_fields(head: &str) -> TestResult<(&str, Vec<(String, String)>)> {
        let lines = head
            .strip_suffix("\r\n\r\n")
            .ok_or_else(|| format!("{head:?} does not end with a blank line"))?;
        let mut lines = lines.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut fields = lines
            .map(|line| {
                line.split_once(": ")
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
                    .ok_or_else(|| format!("{line:?} is not a header line"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.sort();
        Ok((request_line, fields))
    }

    /// A request head of the Rust fetch as [`head_fields`] reads it, without the `Accept: */*`
    /// reqwest adds (http-client.default-accept); hyper writes the names in lower case, in
    /// another order (http-client.header-case and http-client.header-order).
    fn rust_head_fields<'h>(
        head: &'h str,
        what: &str,
    ) -> TestResult<(&'h str, Vec<(String, String)>)> {
        let (request_line, mut fields) = head_fields(head)?;
        let accept = ("accept".to_owned(), "*/*".to_owned());
        let at = fields
            .iter()
            .position(|field| *field == accept)
            .ok_or_else(|| format!("{what}: no Accept in {head:?}"))?;
        let _accept = fields.remove(at);
        Ok((request_line, fields))
    }

    #[test]
    fn fetch_like_go() -> TestResult {
        // Go: main.go:72 (the converter fetches with &http.Client{}: http.DefaultTransport, the
        // proxy environment and no timeout of its own)
        let scenarios = fetch_golden()?;
        let replayed = replay(&scenarios)?;
        check_eq(&replayed.len(), &scenarios.len(), "scenarios")?;
        let successes = |runs: &[Run]| runs.iter().map(|run| run.error.is_none()).collect();
        for (scenario, rust) in scenarios.iter().zip(&replayed) {
            let name = &scenario.name;
            // fetch_urls_differ and fetch_credentials_differ compare these.
            if url_difference(name).is_some() || credentials(name).is_some() {
                continue;
            }
            check_eq::<Vec<bool>>(
                &successes(&rust.runs),
                &successes(&scenario.runs),
                &format!("{name}: the calls that succeed"),
            )?;
            check_eq(
                &rust.constant,
                &scenario.constant,
                &format!("{name}: the constant rates after the calls"),
            )?;
            check_eq(
                &rust.heads.len(),
                &scenario.heads.len(),
                &format!(
                    "{name}: requests {:?}, Go's {:?}",
                    rust.heads, scenario.heads
                ),
            )?;
            for (rust_head, go_head) in rust.heads.iter().zip(&scenario.heads) {
                let (rust_line, rust_fields) = rust_head_fields(rust_head, name)?;
                let (go_line, go_fields) = head_fields(go_head)?;
                check_eq(rust_line, go_line, &format!("{name}: the request line"))?;
                check_eq(
                    &rust_fields,
                    &go_fields,
                    &format!("{name}: the header fields"),
                )?;
            }
        }
        Ok(())
    }

    /// What Go's fetch does with a URL the url crate reads otherwise.
    #[derive(Debug, Clone, Copy)]
    enum GoFetch {
        /// It fails with an error that matches this, `*` standing for net/url's or net/http's
        /// words.
        Refuses(&'static str),
        /// It sends this request line.
        Sends(&'static str),
    }

    /// The scenarios whose URL the url crate reads otherwise than net/url
    /// (`currency.fetch-url`): the request line the Rust fetch sends, and what Go's does.
    const URL_DIFFERENCES: &[(&str, &str, GoFetch)] = &[
        // url.Parse refuses a leading space; the url crate strips white space and C0 controls
        // around the URL.
        (
            "url_surrounding_space",
            "GET /latest.json HTTP/1.1",
            GoFetch::Refuses("parse \" http://127.0.0.1:PORT/latest.json \": *"),
        ),
        // url.Parse keeps a trailing space, which the request line escapes.
        (
            "url_trailing_space",
            "GET /latest.json HTTP/1.1",
            GoFetch::Sends("GET /latest.json%20 HTTP/1.1"),
        ),
        // url.Parse refuses a control character; the url crate strips one at the end and drops
        // a tab or a newline inside.
        (
            "url_trailing_newline",
            "GET /latest.json HTTP/1.1",
            GoFetch::Refuses("parse \"http://127.0.0.1:PORT/latest.json\\n\": *"),
        ),
        (
            "url_tab",
            "GET /latest.json HTTP/1.1",
            GoFetch::Refuses("parse \"http://127.0.0.1:PORT/lat\\test.json\": *"),
        ),
        // Go escapes a backslash in the path; the url crate reads it as a slash.
        (
            "url_backslash",
            "GET /rates/latest.json HTTP/1.1",
            GoFetch::Sends("GET /rates%5Clatest.json HTTP/1.1"),
        ),
        // client.Do refuses `http:<host>` for want of a host; the url crate reads it as
        // `http://<host>`.
        (
            "url_no_slashes",
            "GET /latest.json HTTP/1.1",
            GoFetch::Refuses("Get \"http:127.0.0.1:PORT/latest.json\": *"),
        ),
        // url.Parse refuses a malformed escape; the url crate keeps it.
        (
            "url_bad_escape",
            "GET /%zz.json HTTP/1.1",
            GoFetch::Refuses("parse \"http://127.0.0.1:PORT/%zz.json\": *"),
        ),
        // Go sends dot segments as written; the url crate resolves them.
        (
            "url_dot_segments",
            "GET /latest.json HTTP/1.1",
            GoFetch::Sends("GET /rates/../latest.json HTTP/1.1"),
        ),
    ];

    /// The request line of the Rust fetch and Go's outcome, for a scenario of
    /// [`URL_DIFFERENCES`].
    fn url_difference(scenario: &str) -> Option<(&'static str, GoFetch)> {
        URL_DIFFERENCES
            .iter()
            .find(|(name, ..)| *name == scenario)
            .map(|(_, rust, go)| (*rust, *go))
    }

    /// The scenarios whose URL has credentials (`currency.fetch-credentials`): the request line
    /// the Rust fetch sends, without an `Authorization` field, and the request line and the
    /// `Authorization` field Go's sends.
    const CREDENTIALS: &[(&str, &str, &str, &str)] = &[
        (
            "url_credentials",
            "GET /latest.json HTTP/1.1",
            "GET /latest.json HTTP/1.1",
            // `user:p@ss`, the password's escape decoded.
            "Basic dXNlcjpwQHNz",
        ),
        // To the proxy, the absolute-form request-target holds the credentials.
        (
            "proxy_credentials",
            "GET http://user:pass@currency.test/latest.json HTTP/1.1",
            "GET http://currency.test/latest.json HTTP/1.1",
            "Basic dXNlcjpwYXNz",
        ),
    ];

    /// The request lines and Go's `Authorization` for a scenario of [`CREDENTIALS`].
    fn credentials(scenario: &str) -> Option<(&'static str, &'static str, &'static str)> {
        CREDENTIALS
            .iter()
            .find(|(name, ..)| *name == scenario)
            .map(|(_, rust, go, authorization)| (*rust, *go, *authorization))
    }

    /// Checks that every call of a scenario's Rust fetch succeeded, without a line, and that the
    /// fetch sent one request; returns that request's line and fields.
    fn rust_fetched<'r>(
        name: &str,
        rust: &'r Replayed,
    ) -> TestResult<(&'r str, Vec<(String, String)>)> {
        for run in &rust.runs {
            check_eq(run, &Run::default(), &format!("{name}: the Rust fetch"))?;
        }
        check(
            !rust.constant,
            &format!("{name}: the Rust fetch keeps rates"),
        )?;
        let [head] = rust.heads.as_slice() else {
            return Err(format!("{name}: the Rust fetch sent {:?}", rust.heads).into());
        };
        rust_head_fields(head, name)
    }

    #[test]
    fn fetch_urls_differ() -> TestResult {
        // Go: currency/rate_converter.go:53-58 (http.NewRequestWithContext parses the URL with
        // url.Parse, and client.Do sends its RequestURI)
        let scenarios = fetch_golden()?;
        let replayed = replay(&scenarios)?;
        let ok = scenarios
            .iter()
            .find(|scenario| scenario.name == "ok")
            .ok_or("no scenario ok")?;
        let [ok_head] = ok.heads.as_slice() else {
            return Err(format!("ok: Go sent {:?}", ok.heads).into());
        };
        let (_, plain_fields) = head_fields(ok_head)?;
        let mut checked = 0_usize;
        for (scenario, rust) in scenarios.iter().zip(&replayed) {
            let name = &scenario.name;
            let Some((rust_line, go)) = url_difference(name) else {
                continue;
            };
            let (rust_sent, rust_fields) = rust_fetched(name, rust)?;
            check_eq(
                rust_sent,
                rust_line,
                &format!("{name}: the Rust request line"),
            )?;
            check_eq(
                &rust_fields,
                &plain_fields,
                &format!("{name}: the Rust fields"),
            )?;
            match go {
                GoFetch::Refuses(pattern) => {
                    let [run] = scenario.runs.as_slice() else {
                        return Err(format!("{name}: Go ran {:?}", scenario.runs).into());
                    };
                    let error = run.error.as_deref().unwrap_or_default();
                    check(
                        wildcard(pattern, error).is_some(),
                        &format!("{name}: Go's {error:?} is not {pattern:?}"),
                    )?;
                    check_eq(
                        &run.logs,
                        &vec![("E".to_owned(), format!("{}{error}", UPDATE[1]))],
                        &format!("{name}: Go's lines"),
                    )?;
                    check(
                        scenario.constant && scenario.heads.is_empty(),
                        &format!("{name}: Go's fetch sent {:?}", scenario.heads),
                    )?;
                }
                GoFetch::Sends(go_line) => {
                    let [head] = scenario.heads.as_slice() else {
                        return Err(format!("{name}: Go sent {:?}", scenario.heads).into());
                    };
                    let (go_sent, go_fields) = head_fields(head)?;
                    check_eq(go_sent, go_line, &format!("{name}: Go's request line"))?;
                    check_eq(&go_fields, &plain_fields, &format!("{name}: Go's fields"))?;
                    check(go_line != rust_line, &format!("{name}: Go's request line"))?;
                }
            }
            checked += 1;
        }
        check_eq(
            &checked,
            &URL_DIFFERENCES.len(),
            "the scenarios of the golden",
        )
    }

    #[test]
    fn fetch_credentials_differ() -> TestResult {
        // Go: net/http/client.go send (a URL's userinfo goes out as `Authorization: Basic`) and
        // net/http/request.go Request.write (the request-target to a proxy without it), on
        // main.go:72's &http.Client{}
        let scenarios = fetch_golden()?;
        let replayed = replay(&scenarios)?;
        let mut checked = 0_usize;
        for (scenario, rust) in scenarios.iter().zip(&replayed) {
            let name = &scenario.name;
            let Some((rust_line, go_line, authorization)) = credentials(name) else {
                continue;
            };
            let (rust_sent, rust_fields) = rust_fetched(name, rust)?;
            check_eq(
                rust_sent,
                rust_line,
                &format!("{name}: the Rust request line"),
            )?;
            check(
                rust_fields
                    .iter()
                    .all(|(field, _)| field != "authorization"),
                &format!("{name}: the Rust fetch sent {rust_fields:?}"),
            )?;
            check(
                scenario.runs.iter().all(|run| *run == Run::default()) && !scenario.constant,
                &format!("{name}: Go's fetch failed: {:?}", scenario.runs),
            )?;
            let [head] = scenario.heads.as_slice() else {
                return Err(format!("{name}: Go sent {:?}", scenario.heads).into());
            };
            let (go_sent, mut go_fields) = head_fields(head)?;
            check_eq(go_sent, go_line, &format!("{name}: Go's request line"))?;
            let basic = ("authorization".to_owned(), authorization.to_owned());
            let at = go_fields
                .iter()
                .position(|field| *field == basic)
                .ok_or_else(|| format!("{name}: no {authorization:?} in {head:?}"))?;
            let _basic = go_fields.remove(at);
            check_eq(
                &rust_fields,
                &go_fields,
                &format!("{name}: the other fields"),
            )?;
            checked += 1;
        }
        check_eq(&checked, &CREDENTIALS.len(), "the scenarios of the golden")
    }
}
