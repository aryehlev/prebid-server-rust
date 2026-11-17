//! Go `golang.org/x/net/http/httpproxy` and `http.ProxyFromEnvironment`: `HTTP_PROXY`,
//! `HTTPS_PROXY` and `NO_PROXY` (and their lowercase forms), read once per process.

use std::{env, sync::OnceLock};

use pbs_gocompat::url::Url;
use pbs_util::iputil::net::{Ip, IpNet, join_host_port, parse_cidr, parse_ip, split_host_port};

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

/// Go: `getEnvAny`.
fn get_env_any(names: &[&str]) -> String {
    names
        .iter()
        .filter_map(|n| env::var(n).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

impl Config {
    /// Go: `httpproxy.FromEnvironment()`.
    #[must_use]
    pub fn from_environment() -> Self {
        Self {
            http_proxy: get_env_any(&["HTTP_PROXY", "http_proxy"]),
            https_proxy: get_env_any(&["HTTPS_PROXY", "https_proxy"]),
            no_proxy: get_env_any(&["NO_PROXY", "no_proxy"]),
            cgi: env::var("REQUEST_METHOD").is_ok_and(|v| !v.is_empty()),
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
            let p = p.trim().to_lowercase();
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
        // Go: net.ParseIP, which rejects zones.
        let ip = parse_ip(host);
        if ip.as_ref().is_some_and(is_loopback) {
            return false;
        }
        let lower = host.trim().to_lowercase();
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
}
