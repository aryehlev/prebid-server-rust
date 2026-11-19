//! Pins of the `psl` crate against `golang.org/x/net/publicsuffix` as the auction endpoint
//! derives `site.publisher.domain` (`testdata/golden/psl.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{TestResult, check_table, golden, list, text};

    /// `publicsuffix.EffectiveTLDPlusOne` matches the list case-sensitively and treats a port as
    /// part of the last label, so an upper-case or ported host falls to the default `*` rule;
    /// `psl::domain_str` answers the same for every golden host.
    // Go: endpoints/openrtb2/auction.go:1609 (publicsuffix.EffectiveTLDPlusOne(parsedUrl.Host) for
    // site.publisher.domain)
    #[test]
    fn same_as_go_case_and_port() -> TestResult {
        let doc = golden("psl.json")?;
        let mut rows = Vec::new();
        for case in list(&doc, "/hosts")? {
            let host = text(case, "/host")?;
            let go = if text(case, "/err")?.is_empty() {
                text(case, "/domain")?.to_owned()
            } else {
                "error".to_owned()
            };
            rows.push(vec![
                host.to_owned(),
                go,
                psl::domain_str(host).unwrap_or("error").to_owned(),
            ]);
        }
        check_table(
            &rows,
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
}
