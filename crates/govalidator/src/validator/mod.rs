//! Glue (Rule B (c)): Go: govalidator's `validator.go` and `patterns.go`, as far as `IsURL`,
//! `IsRequestURL`, `IsRequestURI` and `IsIP` need them (url.govalidator-checks).

mod url;

use std::sync::LazyLock;

use pbs_util::{goregex, iputil::net};
use regex::Regex;

/// Go: `maxURLRuneCount`: [`is_url`] refuses a string of this many characters or more.
const MAX_URL_RUNE_COUNT: usize = 2083;

/// Go: `minURLRuneCount`: [`is_url`] refuses a string of this many bytes or fewer.
const MIN_URL_RUNE_COUNT: usize = 3;

/// Go: `URL` in govalidator's `patterns.go`, the pattern `IsURL` matches.
///
/// It is an optional scheme (`URLSchema`) and user (`URLUsername`), a dotted IPv4 address
/// (`URLIP`), a bracketed IPv6 address (`IP`) or a host name (`URLSubdomain` and the labels
/// after it), then an optional port (`URLPort`) and path (`URLPath`).
pub const URL: &str = concat!(
    r"^((ftp|tcp|udp|wss?|https?):\/\/)?(\S+(:\S*)?@)?((([1-9]\d?|1\d\d|2[01]\d|22[0-3]|24\d|",
    r"25[0-5])(\.(\d{1,2}|1\d\d|2[0-4]\d|25[0-5])){2}(?:\.([0-9]\d?|1\d\d|2[0-4]\d|25[0-5]))|",
    r"(\[(([0-9a-fA-F]{1,4}:){7,7}[0-9a-fA-F]{1,4}|([0-9a-fA-F]{1,4}:){1,7}:|",
    r"([0-9a-fA-F]{1,4}:){1,6}:[0-9a-fA-F]{1,4}|([0-9a-fA-F]{1,4}:){1,5}(:[0-9a-fA-F]{1,4}){1,2}|",
    r"([0-9a-fA-F]{1,4}:){1,4}(:[0-9a-fA-F]{1,4}){1,3}|",
    r"([0-9a-fA-F]{1,4}:){1,3}(:[0-9a-fA-F]{1,4}){1,4}|",
    r"([0-9a-fA-F]{1,4}:){1,2}(:[0-9a-fA-F]{1,4}){1,5}|",
    r"[0-9a-fA-F]{1,4}:((:[0-9a-fA-F]{1,4}){1,6})|",
    r":((:[0-9a-fA-F]{1,4}){1,7}|:)|fe80:(:[0-9a-fA-F]{0,4}){0,4}%[0-9a-zA-Z]{1,}|",
    r"::(ffff(:0{1,4}){0,1}:){0,1}((25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}",
    r"(25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9])|([0-9a-fA-F]{1,4}:){1,4}:",
    r"((25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9]))",
    r"\])|(([a-zA-Z0-9]([a-zA-Z0-9-_]+)?[a-zA-Z0-9]([-\.][a-zA-Z0-9]+)*)|",
    r"(((www\.)|([a-zA-Z0-9]+([-_\.]?[a-zA-Z0-9])*[a-zA-Z0-9]\.[a-zA-Z0-9]+))?))?",
    r"(([a-zA-Z\x{00a1}-\x{ffff}0-9]+-?-?)*[a-zA-Z\x{00a1}-\x{ffff}0-9]+)",
    r"(?:\.([a-zA-Z\x{00a1}-\x{ffff}]{1,}))?))\.?(:(\d{1,5}))?((\/|\?|#)[^\s]*)?$",
);

/// Go: `rxURL`, compiled on first use. Go's `MustCompile` of [`URL`] succeeds, and a test
/// checks that `goregex` compiles it too.
static RX_URL: LazyLock<Option<Regex>> = LazyLock::new(|| goregex::compile(URL).ok());

/// Go: `IsURL`: whether `s` is a URL.
///
/// `s` must be shorter than 2083 characters and longer than 3 bytes and must not start with a
/// dot; `url.Parse` must accept it (with `http://` put in front of it when it has a colon but
/// no `://`) and find a host that does not start with a dot, or no host and a path that is
/// empty or holds a dot; and `s` must match [`URL`].
#[must_use]
pub fn is_url(s: &str) -> bool {
    if s.is_empty()
        || s.chars().count() >= MAX_URL_RUNE_COUNT
        || s.len() <= MIN_URL_RUNE_COUNT
        || s.starts_with('.')
    {
        return false;
    }
    let parsed = if s.contains(':') && !s.contains("://") {
        url::parse(&format!("http://{s}"))
    } else {
        url::parse(s)
    };
    let Some(parsed) = parsed else {
        return false;
    };
    if parsed.host.starts_with(b".") {
        return false;
    }
    if parsed.host.is_empty() && !parsed.path.is_empty() && !parsed.path.contains(&b'.') {
        return false;
    }
    RX_URL.as_ref().is_some_and(|rx| rx.is_match(s))
}

/// Go: `IsRequestURL`: whether `raw`, as an HTTP request would carry it, is an absolute URL:
/// `url.ParseRequestURI` accepts it and finds a scheme.
#[must_use]
pub fn is_request_url(raw: &str) -> bool {
    url::parse_request_uri(raw).is_some_and(|parsed| !parsed.scheme.is_empty())
}

/// Go: `IsRequestURI`: whether `raw`, as an HTTP request would carry it, is an absolute URI or
/// an absolute path: `url.ParseRequestURI` accepts it.
#[must_use]
pub fn is_request_uri(raw: &str) -> bool {
    url::parse_request_uri(raw).is_some()
}

/// Go: `IsIP`: whether `s` is an IPv4 or IPv6 address as `net.ParseIP` reads one (dotted
/// decimal without leading zeros, IPv6 with an optional embedded IPv4 address, and no zone).
#[must_use]
pub fn is_ip(s: &str) -> bool {
    net::parse_ip(s).is_some()
}
