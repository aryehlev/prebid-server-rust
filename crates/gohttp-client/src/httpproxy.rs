//! Go `golang.org/x/net/http/httpproxy` and `http.ProxyFromEnvironment`: `HTTP_PROXY`,
//! `HTTPS_PROXY` and `NO_PROXY` (and their lowercase forms), read once per process.

use std::{env, sync::OnceLock};

use pbs_gocompat::{
    strings::{to_lower, trim_space},
    url::Url,
};
use pbs_util::iputil::{
    net::{Ip, IpNet, join_host_port, parse_cidr, parse_ip, split_host_port},
    netip::{self, Addr},
};

use crate::{error::HttpError, httpguts::idna_to_ascii, request::Request};

/// Go: `httpproxy.Config`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Go: `HTTPProxy`.
    pub http_proxy: String,
    /// Go: `HTTPSProxy`.
    pub https_proxy: String,
    /// Go: `NoProxy`.
    pub no_proxy: String,
    /// Go: `CGI`: refuse `HTTP_PROXY` when running as a CGI script.
    pub cgi: bool,
}

#[derive(Debug, Clone)]
enum Matcher {
    All,
    Cidr(IpNet),
    Ip {
        ip: Ip,
        port: String,
    },
    Domain {
        host: String,
        port: String,
        match_host: bool,
    },
}

/// Go: the `config` a `ProxyFunc` closes over.
#[derive(Debug, Clone, Default)]
pub struct ProxySelector {
    cgi: bool,
    http_proxy: Option<Url>,
    https_proxy: Option<Url>,
    ip_matchers: Vec<Matcher>,
    domain_matchers: Vec<Matcher>,
}

/// Go: `getEnvAny` over `getenv`.
fn get_env_any<F: Fn(&str) -> Option<String>>(getenv: &F, names: &[&str]) -> String {
    names
        .iter()
        .filter_map(|n| getenv(n))
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

impl Config {
    /// Go: `httpproxy.FromEnvironment()`.
    #[must_use]
    pub fn from_environment() -> Self {
        Self::from_env_lookup(|name| env::var(name).ok())
    }

    /// Go: `httpproxy.FromEnvironment()` with `getenv` in place of `os.Getenv` (`None` for an
    /// unset variable).
    #[must_use]
    pub fn from_env_lookup<F: Fn(&str) -> Option<String>>(getenv: F) -> Self {
        Self {
            http_proxy: get_env_any(&getenv, &["HTTP_PROXY", "http_proxy"]),
            https_proxy: get_env_any(&getenv, &["HTTPS_PROXY", "https_proxy"]),
            no_proxy: get_env_any(&getenv, &["NO_PROXY", "no_proxy"]),
            cgi: getenv("REQUEST_METHOD").is_some_and(|v| !v.is_empty()),
        }
    }

    /// Go: `Config.ProxyFunc()`.
    #[must_use]
    pub fn proxy_selector(&self) -> ProxySelector {
        let mut sel = ProxySelector {
            cgi: self.cgi,
            http_proxy: parse_proxy(&self.http_proxy),
            https_proxy: parse_proxy(&self.https_proxy),
            ..ProxySelector::default()
        };
        for p in self.no_proxy.split(',') {
            // Go: strings.ToLower(strings.TrimSpace(p)), the simple case mapping per rune.
            let p = to_lower(trim_space(p));
            if p.is_empty() {
                continue;
            }
            if p == "*" {
                sel.ip_matchers = vec![Matcher::All];
                sel.domain_matchers = vec![Matcher::All];
                return sel;
            }
            if let Ok((_, net)) = parse_cidr(&p) {
                sel.ip_matchers.push(Matcher::Cidr(net));
                continue;
            }
            let (mut phost, pport) = match split_host_port(&p) {
                Ok((h, port)) => {
                    if h.is_empty() {
                        continue;
                    }
                    let h = h
                        .strip_prefix('[')
                        .and_then(|x| x.strip_suffix(']'))
                        .unwrap_or(h);
                    (h.to_owned(), port.to_owned())
                }
                Err(_no_port) => (p.clone(), String::new()),
            };
            if let Some(ip) = parse_ip(&phost) {
                sel.ip_matchers.push(Matcher::Ip { ip, port: pport });
                continue;
            }
            if phost.is_empty() {
                continue;
            }
            if let Some(rest) = phost.strip_prefix("*.") {
                phost = format!(".{rest}");
            }
            let mut match_host = false;
            if !phost.starts_with('.') {
                match_host = true;
                phost = format!(".{phost}");
            }
            if let Ok(v) = idna_to_ascii(&phost) {
                phost = v;
            }
            sel.domain_matchers.push(Matcher::Domain {
                host: phost,
                port: pport,
                match_host,
            });
        }
        sel
    }
}

/// Go: `parseProxy`, whose errors `init` ignores.
fn parse_proxy(proxy: &str) -> Option<Url> {
    if proxy.is_empty() {
        return None;
    }
    match Url::parse(proxy) {
        Ok(u) if !u.scheme.is_empty() && !u.host.is_empty() => Some(u),
        parsed => match Url::parse(&format!("http://{proxy}")) {
            Ok(u) => Some(u),
            Err(_retry) => parsed.ok(),
        },
    }
}

/// Go: `netip.ParseAddr(host)` then `net.IP(nip.AsSlice())`, as `useProxy` reads a host: unlike
/// `net.ParseIP` it accepts an IPv6 address with a zone (`fe80::1%en0`), which the IP drops.
fn parse_addr_ip(host: &str) -> Option<Ip> {
    match netip::parse_addr(host).ok()? {
        Addr::V4(octets) => Ip::from_slice(&octets),
        Addr::V6 { octets, .. } => Some(Ip::from16(octets)),
        Addr::Invalid => None,
    }
}

/// Go: `net.IP.IsLoopback`.
fn is_loopback(ip: &Ip) -> bool {
    let mut v6_loopback = [0_u8; 16];
    if let Some(last) = v6_loopback.last_mut() {
        *last = 1;
    }
    ip.to4().map_or_else(
        || ip.equal(&Ip::from16(v6_loopback)),
        |v4| v4.as_slice().first() == Some(&127),
    )
}

/// Go: `httpproxy.canonicalAddr`.
fn canonical_addr(url: &Url) -> String {
    let host = url.hostname();
    let addr = idna_to_ascii(host).unwrap_or_else(|_err| host.to_owned());
    let port = match url.port() {
        "" => match url.scheme.as_str() {
            "http" => "80",
            "https" => "443",
            "socks5" => "1080",
            _ => "",
        },
        p => p,
    };
    join_host_port(&addr, port)
}

impl Matcher {
    fn matches(&self, host: &str, port: &str, ip: Option<&Ip>) -> bool {
        match self {
            Self::All => true,
            Self::Cidr(net) => ip.is_some_and(|ip| net.contains(ip)),
            Self::Ip { ip: m, port: p } => {
                ip.is_some_and(|ip| m.equal(ip)) && (p.is_empty() || p == port)
            }
            Self::Domain {
                host: m,
                port: p,
                match_host,
            } => {
                if ip.is_some() {
                    return false;
                }
                (host.ends_with(m.as_str())
                    || (*match_host && m.strip_prefix('.').is_some_and(|h| h == host)))
                    && (p.is_empty() || p == port)
            }
        }
    }
}

impl ProxySelector {
    /// Go: `useProxy(addr)`.
    fn use_proxy(&self, addr: &str) -> bool {
        if addr.is_empty() {
            return true;
        }
        let Ok((host, port)) = split_host_port(addr) else {
            return false;
        };
        if host == "localhost" {
            return false;
        }
        let ip = parse_addr_ip(host);
        if ip.as_ref().is_some_and(is_loopback) {
            return false;
        }
        let lower = to_lower(trim_space(host));
        if ip.is_some()
            && self
                .ip_matchers
                .iter()
                .any(|m| m.matches(&lower, port, ip.as_ref()))
        {
            return false;
        }
        !self
            .domain_matchers
            .iter()
            .any(|m| m.matches(&lower, port, ip.as_ref()))
    }

    /// Go: `proxyForURL`.
    ///
    /// # Errors
    ///
    /// Go's CGI refusal: `refusing to use HTTP_PROXY value in CGI environment; see
    /// golang.org/s/cgihttpproxy`.
    pub fn proxy_for_url(&self, req_url: &Url) -> Result<Option<Url>, HttpError> {
        let proxy = match req_url.scheme.as_str() {
            "https" => self.https_proxy.as_ref(),
            "http" => {
                if self.http_proxy.is_some() && self.cgi {
                    return Err(HttpError::Static(
                        "refusing to use HTTP_PROXY value in CGI environment; see \
                         golang.org/s/cgihttpproxy",
                    ));
                }
                self.http_proxy.as_ref()
            }
            _ => None,
        };
        let Some(proxy) = proxy else {
            return Ok(None);
        };
        if !self.use_proxy(&canonical_addr(req_url)) {
            return Ok(None);
        }
        Ok(Some(proxy.clone()))
    }
}

/// Go: `http.ProxyFromEnvironment(req)`: the environment is read on first use only.
///
/// # Errors
///
/// As [`ProxySelector::proxy_for_url`].
pub fn proxy_from_environment(req: &Request) -> Result<Option<Url>, HttpError> {
    static SELECTOR: OnceLock<ProxySelector> = OnceLock::new();
    SELECTOR
        .get_or_init(|| Config::from_environment().proxy_selector())
        .proxy_for_url(&req.url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    fn proxied(sel: &ProxySelector, url: &str) -> Result<Option<String>, String> {
        let target = Url::parse(url).map_err(|e| e.to_string())?;
        sel.proxy_for_url(&target)
            .map(|p| p.map(|proxy| proxy.to_string()))
            .map_err(|e| e.to_string())
    }

    #[test]
    fn no_proxy_rules() -> Result<(), String> {
        let sel = Config {
            http_proxy: "proxy.example:3128".to_owned(),
            https_proxy: "https://secure.example".to_owned(),
            no_proxy: "foo.com, .bar.com, 10.0.0.0/8, 1.2.3.4:81, *.wild.org".to_owned(),
            cgi: false,
        }
        .proxy_selector();
        let p = Some("http://proxy.example:3128".to_owned());
        ensure_eq(&proxied(&sel, "http://example.org/")?, &p, "proxied")?;
        ensure_eq(&proxied(&sel, "http://foo.com/")?, &None, "host match")?;
        ensure_eq(&proxied(&sel, "http://sub.foo.com/")?, &None, "subdomain")?;
        ensure_eq(
            &proxied(&sel, "http://bar.com/")?,
            &p,
            "dot prefix only subdomains",
        )?;
        ensure_eq(&proxied(&sel, "http://x.bar.com/")?, &None, "dot prefix")?;
        ensure_eq(&proxied(&sel, "http://10.1.2.3/")?, &None, "cidr")?;
        ensure_eq(&proxied(&sel, "http://1.2.3.4:81/")?, &None, "ip port")?;
        ensure_eq(&proxied(&sel, "http://1.2.3.4/")?, &p, "ip other port")?;
        ensure_eq(&proxied(&sel, "http://localhost/")?, &None, "localhost")?;
        ensure_eq(&proxied(&sel, "http://127.0.0.2/")?, &None, "loopback")?;
        ensure_eq(&proxied(&sel, "http://a.wild.org/")?, &None, "wildcard")?;
        ensure_eq(
            &proxied(&sel, "https://example.org/")?,
            &Some("https://secure.example".to_owned()),
            "https proxy",
        )
    }

    #[test]
    fn zones_and_go_case_mapping() -> Result<(), String> {
        let sel = Config {
            http_proxy: "proxy.example:3128".to_owned(),
            no_proxy: "fe80::/10,\u{a0}\u{130}.test\u{a0}".to_owned(),
            ..Config::default()
        }
        .proxy_selector();
        let p = Some("http://proxy.example:3128".to_owned());
        // useProxy parses hosts with netip.ParseAddr, which accepts an IPv6 zone.
        ensure_eq(
            &proxied(&sel, "http://[::1%25lo0]:8080/")?,
            &None,
            "zoned loopback",
        )?;
        ensure_eq(
            &proxied(&sel, "http://[fe80::1%25en0]/")?,
            &None,
            "zoned address in a NO_PROXY CIDR",
        )?;
        ensure_eq(
            &proxied(&sel, "http://[2001:db8::1%25eth0]/")?,
            &p,
            "zoned address outside the CIDR",
        )?;
        // strings.TrimSpace drops the no-break spaces and strings.ToLower maps U+0130 to i.
        ensure_eq(&proxied(&sel, "http://i.test/")?, &None, "simple lowering")?;
        ensure_eq(&proxied(&sel, "http://a.i.test/")?, &None, "subdomain")?;
        let env = Config::from_env_lookup(|name| match name {
            "HTTP_PROXY" => Some(String::new()),
            "http_proxy" => Some("lower.example".to_owned()),
            "HTTPS_PROXY" => Some("upper.example".to_owned()),
            "https_proxy" => Some("ignored.example".to_owned()),
            "REQUEST_METHOD" => Some("GET".to_owned()),
            _ => None,
        });
        ensure_eq(
            &env,
            &Config {
                http_proxy: "lower.example".to_owned(),
                https_proxy: "upper.example".to_owned(),
                no_proxy: String::new(),
                cgi: true,
            },
            "environment precedence",
        )
    }
}
