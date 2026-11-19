//! Pins of the `psl` crate against `golang.org/x/net/publicsuffix` as the auction endpoint
//! derives `site.publisher.domain` (`testdata/golden/psl.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{BoxError, TestResult, check_table, golden, list, text};
    use serde_json::Value;

    /// Per golden lookup at `pointer`: the host, what `EffectiveTLDPlusOne` gave (`error` when
    /// it failed) and what `psl::domain_str` gives (`none` when it finds no domain).
    fn rows(doc: &Value, pointer: &str) -> Result<Vec<Vec<String>>, BoxError> {
        list(doc, pointer)?
            .iter()
            .map(|case| {
                let host = text(case, "/host")?;
                let go = if text(case, "/err")?.is_empty() {
                    text(case, "/domain")?.to_owned()
                } else {
                    "error".to_owned()
                };
                Ok(vec![
                    host.to_owned(),
                    go,
                    psl::domain_str(host).unwrap_or("none").to_owned(),
                ])
            })
            .collect()
    }

    /// `publicsuffix.EffectiveTLDPlusOne` matches the list case-sensitively and treats a port as
    /// part of the last label, so an upper-case or ported host falls to the default `*` rule;
    /// `psl::domain_str` answers the same for every golden host.
    // Go: endpoints/openrtb2/auction.go:1609 (publicsuffix.EffectiveTLDPlusOne(parsedUrl.Host) for
    // site.publisher.domain)
    #[test]
    fn same_as_go_case_and_port() -> TestResult {
        let doc = golden("psl.json")?;
        check_table(
            &rows(&doc, "/hosts")?,
            &[
                &["WWW.EXAMPLE.CO.UK", "CO.UK", "CO.UK"],
                &[
                    "www.example.com:8080",
                    "example.com:8080",
                    "example.com:8080",
                ],
                &["Example.COM", "Example.COM", "Example.COM"],
                &["www.Example.co.uk", "Example.co.uk", "Example.co.uk"],
            ],
            "host, publicsuffix, psl",
        )
    }

    /// `EffectiveTLDPlusOne` fails on a host with an empty label (the trailing dot of a fully
    /// qualified name, a leading dot, two dots in a row) and on a bare IP address, which
    /// `PublicSuffix` returns whole, so Go leaves `site.publisher.domain` unset for such a
    /// referrer. `psl::domain_str` drops the trailing or leading dot and answers `example.com`,
    /// skips an inner empty label (`a..b.example.co.uk` gives `example.co.uk`, and
    /// `example..com` the empty label and its suffix, `.com`), and takes the last two numbers
    /// of an IPv4 address (`192.168.1.1` gives `1.1`), so the domain is set. With a port, both
    /// treat an IPv4 host as a name (`1.1:8080`), and both find no domain in a bracketed IPv6
    /// host.
    // Go: endpoints/openrtb2/auction.go:1607-1611 (url.Parse of the referrer, then
    // publicsuffix.EffectiveTLDPlusOne(parsedUrl.Host) for site.publisher.domain, skipped on an
    // error); x/net v0.55.0's publicsuffix/list.go:173-176 (the empty-label check) and :88-90
    // (PublicSuffix returns an IP address whole)
    #[test]
    fn go_difference_empty_labels_and_ip_addresses() -> TestResult {
        let doc = golden("psl.json")?;
        let referrers: Vec<String> = list(&doc, "/referrers")?
            .iter()
            .map(|case| text(case, "/referrer").map(str::to_owned))
            .collect::<Result<_, _>>()?;
        check_table(
            &[referrers],
            &[&[
                "http://example.com./article",
                "https://www.example.com./",
                "http://.example.com/",
                "http://example..com/",
                "http://a..b.example.co.uk/",
                "http://192.168.1.1/",
                "http://192.168.1.1:8080/",
                "http://[::1]:8080/",
            ]],
            "the referrers url.Parse gave the hosts of",
        )?;
        check_table(
            &rows(&doc, "/referrers")?,
            &[
                &["example.com.", "error", "example.com"],
                &["www.example.com.", "error", "example.com"],
                &[".example.com", "error", "example.com"],
                &["example..com", "error", ".com"],
                &["a..b.example.co.uk", "error", "example.co.uk"],
                &["192.168.1.1", "error", "1.1"],
                &["192.168.1.1:8080", "1.1:8080", "1.1:8080"],
                &["[::1]:8080", "error", "none"],
            ],
            "host, publicsuffix, psl",
        )
    }
}
