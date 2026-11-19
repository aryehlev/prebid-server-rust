//! Pins of the `cookie` crate against Go's `http.Cookie` as `usersync.WriteCookie` writes the
//! `uids` cookie (`testdata/golden/cookie.json`).

#[cfg(test)]
mod tests {
    use cookie::{Cookie, SameSite};
    use pbs_differences::{TestResult, check_table, golden, text};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    /// Go's `http.Cookie.String` writes `Path`, `Domain`, `Expires`, `Secure` and then
    /// `SameSite`; the `cookie` crate writes the same attributes and value (the base64 padding
    /// raw, with no features on) in another order.
    // Go: usersync/cookie.go:93-113 (WriteCookie: Path "/", the host cookie domain, Expires, and
    // Secure with SameSite=None for site cookies)
    #[test]
    fn go_difference_set_cookie_attribute_order() -> TestResult {
        let doc = golden("cookie.json")?;
        let mut cookie = Cookie::new(
            text(&doc, "/name")?.to_owned(),
            text(&doc, "/value")?.to_owned(),
        );
        cookie.set_path(text(&doc, "/path")?.to_owned());
        cookie.set_domain(text(&doc, "/domain")?.to_owned());
        cookie.set_expires(OffsetDateTime::parse(text(&doc, "/expires")?, &Rfc3339)?);
        cookie.set_secure(true);
        cookie.set_same_site(SameSite::None);
        check_table(
            &[vec![
                text(&doc, "/set_cookie")?.to_owned(),
                cookie.to_string(),
            ]],
            &[&[
                "uids=eyJ1aWRzIjp7fX0=; Path=/; Domain=pbs.example; Expires=Wed, 18 Nov 2026 \
                 10:20:30 GMT; Secure; SameSite=None",
                "uids=eyJ1aWRzIjp7fX0=; SameSite=None; Secure; Path=/; Domain=pbs.example; \
                 Expires=Wed, 18 Nov 2026 10:20:30 GMT",
            ]],
            "http.Cookie, cookie",
        )
    }
}
