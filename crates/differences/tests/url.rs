//! Pins of the `url` crate (WHATWG) against Go's `net/url` as the auction endpoint parses the
//! referrer: the host it reports and the URL it writes back (`testdata/golden/url.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{TestResult, check_table, golden, list, text};
    use url::Url;

    /// Go's `url.Parse` keeps `Host` as written (`Example.COM:80`, a non-ASCII host as it came)
    /// and `String()` percent-encodes a non-ASCII host; the WHATWG parser lower-cases the host,
    /// drops the scheme's default port and writes a non-ASCII host in IDNA form. `site.domain`
    /// takes Go's `Host`, port included.
    // Go: endpoints/openrtb2/auction.go:1606-1608 (url.Parse of the referrer; site.domain from
    // parsedUrl.Host)
    #[test]
    fn go_difference_url_host_normalization() -> TestResult {
        let doc = golden("url.json")?;
        let mut rows = Vec::new();
        for case in list(&doc, "/urls")? {
            let input = text(case, "/input")?;
            let parsed = Url::parse(input)?;
            let rust_host = match (parsed.host_str(), parsed.port()) {
                (Some(host), Some(port)) => format!("{host}:{port}"),
                (Some(host), None) => host.to_owned(),
                (None, _) => String::new(),
            };
            rows.push(vec![
                input.to_owned(),
                text(case, "/host")?.to_owned(),
                text(case, "/string")?.to_owned(),
                rust_host,
                parsed.to_string(),
            ]);
        }
        check_table(
            &rows,
            &[
                &[
                    "http://Example.COM:80/x",
                    "Example.COM:80",
                    "http://Example.COM:80/x",
                    "example.com",
                    "http://example.com/x",
                ],
                &[
                    "http://b\u{fc}cher.example/",
                    "b\u{fc}cher.example",
                    "http://b%C3%BCcher.example/",
                    "xn--bcher-kva.example",
                    "http://xn--bcher-kva.example/",
                ],
                &[
                    "https://Pub.Example:443/a?b=c",
                    "Pub.Example:443",
                    "https://Pub.Example:443/a?b=c",
                    "pub.example",
                    "https://pub.example/a?b=c",
                ],
                &[
                    "http://example.com:8080/",
                    "example.com:8080",
                    "http://example.com:8080/",
                    "example.com:8080",
                    "http://example.com:8080/",
                ],
            ],
            "input, Go Host, Go String(), url host[:port], url as a string",
        )
    }
}
