//! Pins of the currency fetch against Go's (`testdata/golden/fetch.golden`, written by
//! `testdata/go/fetch.go`): `RateConverter.Run` on main.go's `&http.Client{}`, against loopback
//! servers that answer with the bytes Go's did. The outcomes, the fallback to the constant rates,
//! the requests, Prebid's error texts and log lines and the deadline's texts are Go's; the words
//! of the client, the body reader and the URL parser are reqwest's and the url crate's where Go
//! has net/http's and net/url's (`currency.fetch-error-texts`).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use crate::support::{Run, TestResult, check, check_eq, fetch_golden, replay};

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
        match scenario {
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
            "unsupported_scheme" => (
                Some(library("Get \"ftp://127.0.0.1/latest.json\": *")),
                None,
            ),
            _ => (None, None),
        }
    }

    /// What the `*` of `pattern` stands for in `text`, at least one byte; empty for a pattern
    /// without one that is `text`.
    fn wildcard<'t>(pattern: &str, text: &'t str) -> Option<&'t str> {
        match pattern.split_once('*') {
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
                let (rust_line, mut rust_fields) = head_fields(rust_head)?;
                let (go_line, go_fields) = head_fields(go_head)?;
                check_eq(rust_line, go_line, &format!("{name}: the request line"))?;
                // reqwest adds `Accept: */*` (http-client.default-accept) and hyper writes the
                // names in lower case, in another order (http-client.header-case and
                // http-client.header-order).
                let accept = ("accept".to_owned(), "*/*".to_owned());
                let at = rust_fields
                    .iter()
                    .position(|field| *field == accept)
                    .ok_or_else(|| format!("{name}: no Accept in {rust_head:?}"))?;
                let _accept = rust_fields.remove(at);
                check_eq(
                    &rust_fields,
                    &go_fields,
                    &format!("{name}: the header fields"),
                )?;
            }
        }
        Ok(())
    }
}
