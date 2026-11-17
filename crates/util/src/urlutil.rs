//! Go `util/urlutil`.

/// Whether `c` is in Go's `[a-zA-Z0-9.-]`.
const fn is_host_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'.' || c == b'-'
}

/// Go: `urlutil.IsSafeHost`: whether `host` matches `^[a-zA-Z0-9.-]+(:[0-9]+)?$`, a bare host
/// name with an optional port.
///
/// It rejects URL control characters such as `/`, `?`, `#` and `@`, so a user-supplied host
/// cannot rewrite an outbound request URL. As in Go's regexp, `$` only matches at the end of
/// the text, so a trailing newline is rejected.
#[must_use]
pub fn is_safe_host(host: &str) -> bool {
    is_safe_host_bytes(host.as_bytes())
}

/// [`is_safe_host`] of a byte string (Go strings need not be UTF-8).
fn is_safe_host_bytes(b: &[u8]) -> bool {
    let name_len = b.iter().take_while(|&&c| is_host_byte(c)).count();
    if name_len == 0 {
        return false;
    }
    match b.get(name_len..).and_then(<[u8]>::split_first) {
        None => true,
        Some((b':', port)) => !port.is_empty() && port.iter().all(u8::is_ascii_digit),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::str;

    use super::{is_safe_host, is_safe_host_bytes};
    use crate::golden::{TestResult, check, check_eq, fields, read_golden};

    #[test]
    fn is_safe_host_go_cases() -> TestResult {
        // Go: TestIsSafeHost.
        let cases = [
            ("hostname", "example.com", true),
            ("subdomain", "api-us", true),
            ("hostname with port", "example.com:8080", true),
            ("empty", "", false),
            ("path injection", "127.0.0.1:6060/debug/pprof", false),
            ("fragment injection", "127.0.0.1:6060#", false),
            ("query injection", "example.com?x=1", false),
            ("userinfo injection", "example.com@127.0.0.1", false),
            ("scheme injection", "http://127.0.0.1", false),
        ];
        for (name, host, want) in cases {
            check_eq(&is_safe_host(host), &want, name)?;
        }
        Ok(())
    }

    #[test]
    fn is_safe_host_golden() -> TestResult {
        // The regexp against curated hosts and random strings of host, URL and white space
        // bytes.
        let rows = read_golden("urlutil.txt")?;
        check(rows.len() > 2000, "urlutil.txt is complete")?;
        for row in &rows {
            let [s, want] = fields::<2>(row)?;
            let safe = str::from_utf8(s).map_or_else(|_| is_safe_host_bytes(s), is_safe_host);
            let got: &[u8] = if safe { b"true" } else { b"false" };
            check_eq(got, want, &String::from_utf8_lossy(s))?;
        }
        Ok(())
    }
}
