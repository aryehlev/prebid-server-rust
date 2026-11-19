//! Glue (Rule B (c)): the operator's proxy environment, read by Go's rules.
//!
//! Go's clients take their proxy from `http.ProxyFromEnvironment`, which is
//! golang.org/x/net/http/httpproxy (v0.55.0): `HTTP_PROXY` for `http` URLs and `HTTPS_PROXY` for
//! `https` ones (the upper-case name first, then the lower-case one), `NO_PROXY`'s hosts, domains,
//! IPs and CIDRs excepted, `localhost` and loopback addresses never proxied, `ALL_PROXY` not read,
//! and `HTTP_PROXY` refused under `REQUEST_METHOD` (a CGI environment). reqwest's own environment
//! rules differ, and a wrong rule could send traffic, parity traffic included, off the machine,
//! so the clients choose through [`ProxyRules`], a port of httpproxy's `config`, behind
//! reqwest's `Proxy::custom` (`http-client.proxy`, restored; pin
//! `crates/http-client/tests/proxy.rs::proxy_env_like_go` against `testdata/golden/proxy.json`).
//!
//! The environment comes in as a [`ProxyEnv`], read once at startup as Go reads it on first use,
//! so tests never set process variables.

use std::{env, net::IpAddr};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use url::{Host, Url};

/// The proxy environment, as Go's `httpproxy.FromEnvironment` reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyEnv {
    /// `HTTP_PROXY`, else `http_proxy`.
    pub http_proxy: String,
    /// `HTTPS_PROXY`, else `https_proxy`.
    pub https_proxy: String,
    /// `NO_PROXY`, else `no_proxy`.
    pub no_proxy: String,
    /// Whether `REQUEST_METHOD` is set: a CGI environment, where `HTTP_PROXY` may come from the
    /// request's `Proxy` header and is refused.
    pub cgi: bool,
}

impl ProxyEnv {
    /// The process environment (a variable that is not UTF-8 counts as unset).
    #[must_use]
    pub fn from_process() -> Self {
        Self::from_lookup(|name| env::var(name).ok())
    }

    /// The environment `getenv` gives (`None` for an unset variable): Go's `getEnvAny`, which
    /// takes the first of the names that is set and not empty.
    #[must_use]
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(getenv: F) -> Self {
        let any = |names: [&str; 2]| {
            names
                .into_iter()
                .filter_map(&getenv)
                .find(|value| !value.is_empty())
                .unwrap_or_default()
        };
        Self {
            http_proxy: any(["HTTP_PROXY", "http_proxy"]),
            https_proxy: any(["HTTPS_PROXY", "https_proxy"]),
            no_proxy: any(["NO_PROXY", "no_proxy"]),
            cgi: getenv("REQUEST_METHOD").is_some_and(|value| !value.is_empty()),
        }
    }

    /// The rules this environment gives: Go's `Config.ProxyFunc`.
    #[must_use]
    pub fn rules(&self) -> ProxyRules {
        ProxyRules::new(self)
    }
}

/// A proxy URL from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUrl {
    /// The text Go's `parseProxy` read: the value as written, or behind `http://` when it had no
    /// scheme or host.
    text: String,
    /// The url crate's reading of `text`, when it has one.
    url: Option<Url>,
}

impl ProxyUrl {
    fn new(text: String) -> Self {
        let url = Url::parse(&text).ok();
        Self { text, url }
    }

    /// The text Go read the proxy from.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The proxy as reqwest reads it.
    #[must_use]
    pub const fn url(&self) -> Option<&Url> {
        self.url.as_ref()
    }

    /// The proxy, when reqwest can send through it: an `http` or `https` URL. Go also dials
    /// `socks5` proxies, which the stack's reqwest does not (`http-client.socks-proxy`).
    #[must_use]
    pub fn usable(&self) -> Option<&Url> {
        self.url
            .as_ref()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
    }
}

/// Where a request goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice<'a> {
    /// Straight to the host.
    Direct,
    /// Through this proxy.
    Proxy(&'a ProxyUrl),
    /// Nowhere: `HTTP_PROXY` in a CGI environment, which Go refuses with an error.
    Refused,
}

/// A `NO_PROXY` entry: Go's matchers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Matcher {
    /// `*`.
    All,
    /// A CIDR, as Go's `IPNet.Contains` reads it.
    Cidr(IpNet),
    /// An IP, with a port when the entry has one.
    Ip { ip: IpAddr, port: String },
    /// A domain with its leading dot (`.example.com`), a port when the entry has one, and
    /// whether the domain itself matches as well as its subdomains.
    Domain {
        host: String,
        port: String,
        match_host: bool,
    },
}

impl Matcher {
    fn matches(&self, host: &str, port: &str, ip: Option<IpAddr>) -> bool {
        match self {
            Self::All => true,
            Self::Cidr(net) => ip.is_some_and(|ip| net.contains(&ip.to_canonical())),
            Self::Ip { ip: entry, port: p } => {
                ip.is_some_and(|ip| entry.to_canonical() == ip.to_canonical())
                    && (p.is_empty() || p == port)
            }
            Self::Domain {
                host: domain,
                port: p,
                match_host,
            } => {
                ip.is_none()
                    && (host.ends_with(domain.as_str())
                        || (*match_host && domain.strip_prefix('.') == Some(host)))
                    && (p.is_empty() || p == port)
            }
        }
    }
}

/// httpproxy's `config`: the proxies and `NO_PROXY`'s matchers; or one proxy for every request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyRules {
    /// Go's `http.ProxyURL`: this proxy for every request, whatever the environment.
    fixed: Option<ProxyUrl>,
    cgi: bool,
    http_proxy: Option<ProxyUrl>,
    https_proxy: Option<ProxyUrl>,
    ip_matchers: Vec<Matcher>,
    domain_matchers: Vec<Matcher>,
}

impl ProxyRules {
    /// The rules of `env`: httpproxy's `config.init`.
    #[must_use]
    pub fn new(env: &ProxyEnv) -> Self {
        let mut rules = Self {
            cgi: env.cgi,
            http_proxy: parse_proxy(&env.http_proxy),
            https_proxy: parse_proxy(&env.https_proxy),
            ..Self::default()
        };
        for entry in env.no_proxy.split(',') {
            let entry = go_to_lower(entry.trim());
            if entry.is_empty() {
                continue;
            }
            if entry == "*" {
                rules.ip_matchers = vec![Matcher::All];
                rules.domain_matchers = vec![Matcher::All];
                return rules;
            }
            if let Some(net) = parse_cidr(&entry) {
                rules.ip_matchers.push(Matcher::Cidr(net));
                continue;
            }
            let (host, port) = match split_host_port(&entry) {
                Some(("", _)) => continue,
                Some((host, port)) => {
                    let host = host
                        .strip_prefix('[')
                        .and_then(|inner| inner.strip_suffix(']'))
                        .unwrap_or(host);
                    (host.to_owned(), port.to_owned())
                }
                None => (entry.clone(), String::new()),
            };
            if let Ok(ip) = host.parse::<IpAddr>() {
                rules.ip_matchers.push(Matcher::Ip { ip, port });
                continue;
            }
            if host.is_empty() {
                continue;
            }
            let host = host
                .strip_prefix('*')
                .filter(|h| h.starts_with('.'))
                .unwrap_or(&host);
            let (host, match_host) = if host.starts_with('.') {
                (host.to_owned(), false)
            } else {
                (format!(".{host}"), true)
            };
            let host = idna_ascii(&host).unwrap_or(host);
            rules.domain_matchers.push(Matcher::Domain {
                host,
                port,
                match_host,
            });
        }
        rules
    }

    /// Every request through `proxy`, loopback ones included, whatever the environment: Go's
    /// `http.ProxyURL(proxy)`, the proxy the parity harness gives its oracle transport
    /// (parity/transport/transport.go:159-173). This is reqwest's `Proxy::all` read through the
    /// same rules as the environment's proxies, so the per-host limit and the loopback guard see
    /// it.
    #[must_use]
    pub fn fixed(proxy: Url) -> Self {
        Self {
            fixed: Some(ProxyUrl {
                text: proxy.to_string(),
                url: Some(proxy),
            }),
            ..Self::default()
        }
    }

    /// Whether the environment names a proxy at all.
    #[must_use]
    pub const fn has_proxies(&self) -> bool {
        self.fixed.is_some() || self.http_proxy.is_some() || self.https_proxy.is_some()
    }

    /// Where a request for `url` goes: httpproxy's `proxyForURL`, or the fixed proxy.
    #[must_use]
    pub fn choose(&self, url: &Url) -> Choice<'_> {
        if let Some(proxy) = &self.fixed {
            return Choice::Proxy(proxy);
        }
        let proxy = match url.scheme() {
            "https" => self.https_proxy.as_ref(),
            "http" => {
                if self.http_proxy.is_some() && self.cgi {
                    return Choice::Refused;
                }
                self.http_proxy.as_ref()
            }
            _ => None,
        };
        match proxy {
            Some(proxy) if self.use_proxy(url) => Choice::Proxy(proxy),
            Some(_) | None => Choice::Direct,
        }
    }

    /// httpproxy's `useProxy` over `canonicalAddr(url)`. The url crate has already lower-cased
    /// the host and turned it to punycode, which `canonicalAddr` does only for non-ASCII hosts.
    fn use_proxy(&self, url: &Url) -> bool {
        let host = match url.host() {
            Some(Host::Domain(domain)) => domain.to_owned(),
            Some(Host::Ipv4(ip)) => ip.to_string(),
            Some(Host::Ipv6(ip)) => ip.to_string(),
            None => String::new(),
        };
        let port = url
            .port_or_known_default()
            .map_or_else(String::new, |port| port.to_string());
        if host == "localhost" {
            return false;
        }
        let ip = host.parse::<IpAddr>().ok();
        if ip.is_some_and(|ip| ip.to_canonical().is_loopback()) {
            return false;
        }
        let lower = go_to_lower(host.trim());
        if ip.is_some()
            && self
                .ip_matchers
                .iter()
                .any(|m| m.matches(&lower, &port, ip))
        {
            return false;
        }
        !self
            .domain_matchers
            .iter()
            .any(|m| m.matches(&lower, &port, ip))
    }
}

/// httpproxy's `parseProxy`, whose errors `init` drops: the value as Go's `url.Parse` reads it,
/// or behind `http://` when it has no scheme or host and that reading parses.
fn parse_proxy(value: &str) -> Option<ProxyUrl> {
    if value.is_empty() {
        return None;
    }
    let parsed = go_url::parse(value);
    if !matches!(&parsed, Some(url) if !url.scheme.is_empty() && !url.host.is_empty()) {
        let with_scheme = format!("http://{value}");
        if go_url::parse(&with_scheme).is_some() {
            return Some(ProxyUrl::new(with_scheme));
        }
    }
    parsed.map(|_| ProxyUrl::new(value.to_owned()))
}

/// Go's `strings.ToLower`: each rune's simple lower-case mapping, which for U+0130 is `i`, where
/// Rust's full mapping gives `i` and a combining dot.
fn go_to_lower(text: &str) -> String {
    text.chars()
        .map(|c| {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(single), None) => single,
                _ if c == '\u{130}' => 'i',
                _ => c,
            }
        })
        .collect()
}

/// httpproxy's `idnaASCII`: the text as is when it is ASCII, else its punycode form (Go:
/// `idna.Lookup.ToASCII`), or `None` when it has none.
fn idna_ascii(text: &str) -> Option<String> {
    if text.is_ascii() {
        return Some(text.to_owned());
    }
    let (dot, name) = text
        .strip_prefix('.')
        .map_or(("", text), |rest| (".", rest));
    match Host::parse(name).ok()? {
        Host::Domain(domain) => Some(format!("{dot}{domain}")),
        Host::Ipv4(_) | Host::Ipv6(_) => None,
    }
}

/// Go's `net.SplitHostPort`, or `None` for its errors.
fn split_host_port(hostport: &str) -> Option<(&str, &str)> {
    let colon = hostport.rfind(':')?;
    let (head, port) = hostport.split_at_checked(colon)?;
    let port = port.strip_prefix(':')?;
    let (host, inner_start, inner_end) = if let Some(bracketed) = head.strip_prefix('[') {
        // The first ']' must sit just before the last ':'.
        let end = hostport.find(']')?;
        if end + 1 != colon {
            return None;
        }
        (bracketed.strip_suffix(']')?, 1, end + 1)
    } else {
        if head.contains(':') {
            return None;
        }
        (head, 0, 0)
    };
    let after_open = hostport.get(inner_start..)?;
    let after_close = hostport.get(inner_end..)?;
    if after_open.contains('[') || after_close.contains(']') {
        return None;
    }
    Some((host, port))
}

/// Go's `net.ParseCIDR`, as `IPNet.Contains` then reads the network. `ParseCIDR` masks the
/// address, so an IPv4-mapped network keeps its `::ffff:` prefix only under a mask of 96 bits or
/// more, and then covers the IPv4 addresses under its last 32 bits of mask; under a shorter mask
/// it is an IPv6 network, which covers no IPv4 address (`Contains` compares an IPv4 address's 4
/// bytes with the network's 16).
fn parse_cidr(text: &str) -> Option<IpNet> {
    let (addr, prefix) = text.split_once('/')?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let prefix: u8 = prefix.parse().ok()?;
    match addr.parse::<IpAddr>().ok()? {
        IpAddr::V4(ip) => Ipv4Net::new(ip, prefix).ok().map(IpNet::V4),
        IpAddr::V6(ip) => {
            let net = Ipv6Net::new(ip, prefix).ok()?;
            match (ip.to_ipv4_mapped(), prefix.checked_sub(96)) {
                (Some(v4), Some(v4_prefix)) => Ipv4Net::new(v4, v4_prefix).ok().map(IpNet::V4),
                _ => Some(IpNet::V6(net)),
            }
        }
    }
}

/// The parts of Go's `url.Parse` (net/url/url.go, Go 1.26.2 with the go 1.25 GODEBUG defaults
/// upstream's go.mod gives) that decide what `parseProxy` returns: whether the text parses, and
/// its scheme and host. `None` stands for Go's errors, whose texts `parseProxy`'s caller drops.
mod go_url {
    use std::{net::Ipv6Addr, str::from_utf8};

    /// What `parseProxy` reads of a parsed URL.
    #[derive(Debug, Default)]
    pub(super) struct Parsed {
        pub(super) scheme: String,
        pub(super) host: String,
    }

    /// The escaping contexts `unescape` validates.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Host,
        Zone,
        Other,
    }

    /// `url.Parse`.
    pub(super) fn parse(raw: &str) -> Option<Parsed> {
        let (rest, fragment) = raw
            .split_once('#')
            .map_or((raw, None), |(u, f)| (u, Some(f)));
        let parsed = parse_without_fragment(rest)?;
        if let Some(fragment) = fragment {
            let _fragment = unescape(fragment, Mode::Other)?;
        }
        Some(parsed)
    }

    /// `parse(rawURL, false)`.
    fn parse_without_fragment(raw: &str) -> Option<Parsed> {
        if raw.bytes().any(|b| b < b' ' || b == 0x7F) {
            return None;
        }
        if raw == "*" {
            return Some(Parsed::default());
        }
        let (scheme, rest) = scheme(raw)?;
        let scheme = scheme.to_ascii_lowercase();
        let rest = match rest.strip_suffix('?') {
            Some(stripped) if rest.matches('?').count() == 1 => stripped,
            _ => rest.split_once('?').map_or(rest, |(before, _query)| before),
        };
        if !rest.starts_with('/') {
            if !scheme.is_empty() {
                // An opaque URL such as `proxy.test:3128`, scheme "proxy.test": no host.
                return Some(Parsed {
                    scheme,
                    host: String::new(),
                });
            }
            let segment = rest.split_once('/').map_or(rest, |(first, _)| first);
            if segment.contains(':') {
                return None;
            }
        }
        let (host, path) = if let Some(after) = rest.strip_prefix("//")
            && (!scheme.is_empty() || !rest.starts_with("///"))
        {
            let (authority, remainder) = after
                .find('/')
                .and_then(|i| after.split_at_checked(i))
                .unwrap_or((after, ""));
            (authority_host(authority)?, remainder)
        } else {
            (String::new(), rest)
        };
        let _path = unescape(path, Mode::Other)?;
        Some(Parsed { scheme, host })
    }

    /// `getScheme`: the scheme and the rest, or `None` for a leading colon.
    fn scheme(raw: &str) -> Option<(&str, &str)> {
        for (i, c) in raw.bytes().enumerate() {
            if c.is_ascii_alphabetic() {
                continue;
            }
            if c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.') {
                if i == 0 {
                    return Some(("", raw));
                }
                continue;
            }
            if c == b':' {
                if i == 0 {
                    return None;
                }
                let (scheme, rest) = raw.split_at_checked(i)?;
                return Some((scheme, rest.strip_prefix(':')?));
            }
            return Some(("", raw));
        }
        Some(("", raw))
    }

    /// `parseAuthority`: the host, after checking the user information.
    fn authority_host(authority: &str) -> Option<String> {
        let (userinfo, hostport) = match authority.rfind('@') {
            Some(at) => {
                let (user, rest) = authority.split_at_checked(at)?;
                (Some(user), rest.strip_prefix('@')?)
            }
            None => (None, authority),
        };
        let host = host(hostport)?;
        if let Some(userinfo) = userinfo {
            if !valid_userinfo(userinfo) {
                return None;
            }
            for part in userinfo.splitn(2, ':') {
                let _part = unescape(part, Mode::Other)?;
            }
        }
        Some(host)
    }

    /// `parseHost`. A host with several colons takes its port after the last one, as Go 1.26
    /// does for every scheme under `urlstrictcolons=0`, the default of upstream's go 1.25 line.
    fn host(host: &str) -> Option<String> {
        match host.rfind('[') {
            Some(0) => {
                let close = host.rfind(']')?;
                let (bracketed, colon_port) = host.split_at_checked(close + 1)?;
                if !valid_optional_port(colon_port) {
                    return None;
                }
                let unescaped_port = unescape(colon_port, Mode::Host)?;
                let name = bracketed.strip_prefix('[')?.strip_suffix(']')?;
                let mut unescaped = match name.find("%25") {
                    Some(zone) => {
                        let (addr, zone) = name.split_at_checked(zone)?;
                        let mut both = unescape(addr, Mode::Host)?;
                        both.extend(unescape(zone, Mode::Zone)?);
                        both
                    }
                    None => unescape(name, Mode::Host)?,
                };
                if !is_ipv6_literal(&unescaped) {
                    return None;
                }
                unescaped.insert(0, b'[');
                unescaped.push(b']');
                unescaped.extend(unescaped_port);
                String::from_utf8(unescaped).ok()
            }
            Some(_) => None,
            None => {
                if let Some(colon) = host.rfind(':') {
                    let (_, colon_port) = host.split_at_checked(colon)?;
                    if !valid_optional_port(colon_port) {
                        return None;
                    }
                }
                String::from_utf8(unescape(host, Mode::Host)?).ok()
            }
        }
    }

    /// `netip.ParseAddr` accepts the bytes, and they are not an IPv4 address.
    fn is_ipv6_literal(bytes: &[u8]) -> bool {
        let Ok(text) = from_utf8(bytes) else {
            return false;
        };
        match text.split_once('%') {
            Some((addr, zone)) => !zone.is_empty() && addr.parse::<Ipv6Addr>().is_ok(),
            None => text.parse::<Ipv6Addr>().is_ok(),
        }
    }

    /// `validOptionalPort`.
    fn valid_optional_port(port: &str) -> bool {
        port.is_empty()
            || port
                .strip_prefix(':')
                .is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
    }

    /// `validUserinfo`.
    fn valid_userinfo(text: &str) -> bool {
        text.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '-' | '.'
                        | '_'
                        | ':'
                        | '~'
                        | '!'
                        | '$'
                        | '&'
                        | '\''
                        | '('
                        | ')'
                        | '*'
                        | '+'
                        | ','
                        | ';'
                        | '='
                        | '%'
                        | '@'
                )
        })
    }

    /// Whether `shouldEscape(c, encodeHost)` holds: the ASCII byte may not appear raw in a host
    /// (`net/url/encoding_table.go`).
    const fn host_byte_escaped(c: u8) -> bool {
        !(c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'!' | b'"'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b'-'
                    | b'.'
                    | b':'
                    | b';'
                    | b'<'
                    | b'='
                    | b'>'
                    | b'['
                    | b']'
                    | b'_'
                    | b'~'
            ))
    }

    /// `unescape`'s validation and decoding.
    fn unescape(text: &str, mode: Mode) -> Option<Vec<u8>> {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while let Some(&c) = bytes.get(i) {
            if c == b'%' {
                let high = bytes.get(i + 1).copied().and_then(hex)?;
                let low = bytes.get(i + 2).copied().and_then(hex)?;
                let is_25 = high == 2 && low == 5;
                let value = (high << 4) | low;
                if mode == Mode::Host && high < 8 && !is_25 {
                    return None;
                }
                if mode == Mode::Zone && !is_25 && value != b' ' && host_byte_escaped(value) {
                    return None;
                }
                out.push(value);
                i += 3;
            } else {
                if matches!(mode, Mode::Host | Mode::Zone) && c < 0x80 && host_byte_escaped(c) {
                    return None;
                }
                out.push(c);
                i += 1;
            }
        }
        Some(out)
    }

    /// A hexadecimal digit's value.
    const fn hex(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{go_to_lower, parse_proxy, split_host_port};

    // Go: golang.org/x/net/http/httpproxy parseProxy, net.SplitHostPort, strings.ToLower.
    #[test]
    fn go_helpers() {
        let text = |value: &str| parse_proxy(value).map(|p| p.text().to_owned());
        assert_eq!(
            text("proxy.test:3128").as_deref(),
            Some("http://proxy.test:3128"),
            "opaque"
        );
        assert_eq!(
            text("http://p.test:1").as_deref(),
            Some("http://p.test:1"),
            "as written"
        );
        assert_eq!(
            text("//p.test:8080").as_deref(),
            Some("http:////p.test:8080"),
            "no scheme"
        );
        assert_eq!(
            text("http://[::1").as_deref(),
            Some("http://http://[::1"),
            "Go retries behind http:// and the retry parses"
        );
        assert_eq!(
            split_host_port("[::1]:80"),
            Some(("::1", "80")),
            "bracketed"
        );
        assert_eq!(split_host_port("a:b:80"), None, "too many colons");
        assert_eq!(split_host_port("a.test"), None, "no port");
        assert_eq!(go_to_lower("\u{130}.TEST"), "i.test", "simple case mapping");
    }
}
