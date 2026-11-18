//! Go 1.26's name constraint checks (`crypto/x509/constraints.go`): after a chain is built, the
//! names of each certificate are checked against the constraints of every certificate above it.
//!
//! Go keeps each constraint list sorted and pruned, and looks a name up by binary search, checking
//! only the constraint just below it in that order; the port keeps that, so it reports the same
//! constraint Go does (Go sorts with `slices.SortFunc`, which is stable for the short lists
//! certificates carry).

use std::{
    cmp::Ordering,
    collections::BTreeMap,
    iter,
    net::{IpAddr, Ipv6Addr},
    str,
};

use pbs_gocompat::{strconv::quote, url::Url};

use super::cert::{Certificate, IpNet, Subtrees};

/// Go: `domainNameValid`: no trailing dot, no empty label (a constraint may start with a dot),
/// and only the bytes 33 to 126. The empty name is valid.
pub(super) fn domain_name_valid(s: &str, constraint: bool) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.ends_with('.') {
        return false;
    }
    let s = if constraint {
        s.strip_prefix('.').unwrap_or(s)
    } else {
        s
    };
    s.split('.')
        .all(|label| !label.is_empty() && label.bytes().all(|b| (33..=126).contains(&b)))
}

/// Go: `domainToReverseLabels(domain)` succeeding.
fn domain_labels_valid(domain: &str) -> bool {
    domain.is_empty()
        || domain
            .split('.')
            .all(|label| !label.is_empty() && label.bytes().all(|b| (33..=126).contains(&b)))
}

/// Go: `rfc2821Mailbox`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mailbox {
    local: String,
    domain: String,
}

impl Mailbox {
    /// Go: `rfc2821Mailbox.String`.
    fn text(&self) -> String {
        format!("{}@{}", self.local, self.domain)
    }
}

/// The local-part bytes RFC 5321 allows without quoting (Go's `NextChar` cases).
const fn atom_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'/'
                | b'='
                | b'?'
                | b'^'
                | b'_'
                | b'`'
                | b'{'
                | b'|'
                | b'}'
                | b'~'
                | b'.'
        )
}

/// Go: `parseRFC2821Mailbox`.
pub(super) fn parse_mailbox(input: &str) -> Option<Mailbox> {
    let mut rest = input.as_bytes();
    let mut local: Vec<u8> = Vec::new();
    if let Some((&b'"', quoted)) = rest.split_first() {
        rest = quoted;
        loop {
            let (&c, after) = rest.split_first()?;
            rest = after;
            match c {
                b'"' => break,
                b'\\' => {
                    let (&escaped, tail) = rest.split_first()?;
                    if !matches!(escaped, 1..=9 | 11 | 12 | 14..=127) {
                        return None;
                    }
                    local.push(escaped);
                    rest = tail;
                }
                1..=8 | 11 | 12 | 14..=31 | 32 | 33 | 35..=91 | 93..=126 | 127 => local.push(c),
                _ => return None,
            }
        }
    } else {
        while let Some((&c, after)) = rest.split_first() {
            if c == b'\\' {
                // Go's `fallthrough` appends the escaped byte, whatever it is.
                let (&escaped, after) = after.split_first()?;
                local.push(escaped);
                rest = after;
            } else if atom_byte(c) {
                local.push(c);
                rest = after;
            } else {
                break;
            }
        }
        if local.first().is_none_or(|&c| c == b'.')
            || local.last() == Some(&b'.')
            || local.windows(2).any(|w| w == b"..")
        {
            return None;
        }
    }
    let (&b'@', domain) = rest.split_first()? else {
        return None;
    };
    let domain = str::from_utf8(domain).ok()?;
    if !domain_labels_valid(domain) {
        return None;
    }
    Some(Mailbox {
        local: String::from_utf8_lossy(&local).into_owned(),
        domain: domain.to_owned(),
    })
}

/// Go: `dnsCompareTable`: ASCII case folded, and `.` below every other byte.
const fn fold(b: u8) -> u8 {
    if b == b'.' { 0 } else { b.to_ascii_lowercase() }
}

/// Go: `dnsCompare`: the names compared from their last byte.
fn dns_compare(a: &str, b: &str) -> Ordering {
    let mut left = a.bytes().rev().map(fold);
    let mut right = b.bytes().rev().map(fold);
    loop {
        match (left.next(), right.next()) {
            (Some(lb), Some(rb)) if lb == rb => {}
            (Some(lb), Some(rb)) => return lb.cmp(&rb),
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

/// Go: `dnsHasSuffix(a, b)`: whether constraint `a` covers name `b` (ASCII case folded; without a
/// leading dot, `a` also covers itself, and only whole labels).
fn dns_has_suffix(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() > b.len() {
        return false;
    }
    let Some(tail) = b.get(b.len() - a.len()..) else {
        return false;
    };
    if !tail.eq_ignore_ascii_case(a) {
        return false;
    }
    if a.first() != Some(&b'.') && b.len() > a.len() {
        // Go indexes a[0] here, which panics for an empty constraint; empty constraints never
        // reach this (they match everything).
        return b.get(b.len() - a.len() - 1) == Some(&b'.');
    }
    true
}

/// Go: `trimFirstLabel`: from the first dot on, or empty.
fn trim_first_label(name: &str) -> &str {
    name.find('.')
        .and_then(|i| name.get(i..))
        .unwrap_or_default()
}

/// Go: `nameConstraintsSet.sortAndPrune`.
fn sort_and_prune<T: Clone>(
    set: &mut Vec<T>,
    cmp: impl Fn(&T, &T) -> Ordering,
    subset: impl Fn(&T, &T) -> bool,
) {
    if set.len() < 2 {
        return;
    }
    set.sort_by(&cmp);
    let mut pruned: Vec<T> = Vec::with_capacity(set.len());
    for item in set.drain(..) {
        if pruned.last().is_none_or(|last| !subset(last, &item)) {
            pruned.push(item);
        }
    }
    *set = pruned;
}

/// Go: `nameConstraintsSet.search`: an exact match, else whether the constraint just below `s` in
/// the set's order (or the first one) matches.
fn search<'a, T, V: ?Sized>(
    set: &'a [T],
    s: &V,
    cmp: impl Fn(&T, &V) -> Ordering,
    matches: impl Fn(&T, &V) -> bool,
) -> Option<&'a T> {
    if set.is_empty() {
        return None;
    }
    let i = set.partition_point(|c| cmp(c, s) == Ordering::Less);
    if let Some(found) = set.get(i).filter(|c| cmp(c, s) == Ordering::Equal) {
        return Some(found);
    }
    let candidate = set.get(i.saturating_sub(1))?;
    matches(candidate, s).then_some(candidate)
}

/// Go: `dnsConstraints` (also used for URI domains and email domains).
#[derive(Debug)]
struct DnsConstraints {
    /// An empty constraint: every name matches.
    all: bool,
    set: Vec<String>,
    /// Go: `parentConstraints`, for excluded lists: wildcard names match a constraint on one of
    /// the names they cover.
    parents: BTreeMap<String, String>,
}

impl DnsConstraints {
    /// Go: `newDNSConstraints`.
    fn new(list: &[String], permitted: bool) -> Option<Self> {
        if list.is_empty() {
            return None;
        }
        if list.iter().any(String::is_empty) {
            return Some(Self {
                all: true,
                set: Vec::new(),
                parents: BTreeMap::new(),
            });
        }
        let mut set = list.to_vec();
        sort_and_prune(
            &mut set,
            |a, b| dns_compare(a, b),
            |a, b| dns_has_suffix(a, b),
        );
        let mut parents = BTreeMap::new();
        if !permitted {
            for name in &set {
                let name = name.to_ascii_lowercase();
                let trimmed = trim_first_label(&name).to_owned();
                if !trimmed.is_empty() {
                    let _previous = parents.insert(trimmed, name);
                }
            }
        }
        Some(Self {
            all: false,
            set,
            parents,
        })
    }

    /// Go: `dnsConstraints.query`: the matching constraint.
    fn query(&self, s: &str) -> Option<String> {
        if self.all {
            return Some(String::new());
        }
        if let Some(found) = search(
            &self.set,
            s,
            |c, s| dns_compare(c, s),
            |c, s| dns_has_suffix(c, s),
        ) {
            return Some(found.clone());
        }
        if !self.parents.is_empty() && s.starts_with('*') {
            let lower = s.to_ascii_lowercase();
            return self.parents.get(trim_first_label(&lower)).cloned();
        }
        None
    }
}

/// Go: `emailConstraints`.
#[derive(Debug)]
struct EmailConstraints {
    domains: Option<DnsConstraints>,
    full: Vec<Mailbox>,
}

impl EmailConstraints {
    /// Go: `newEmailConstraints`.
    fn new(list: &[String], permitted: bool) -> Option<Self> {
        if list.is_empty() {
            return None;
        }
        let mut full = Vec::new();
        let mut domains = Vec::new();
        for c in list {
            if !c.contains('@') {
                domains.push(c.clone());
                continue;
            }
            if let Some(mut parsed) = parse_mailbox(c) {
                parsed.domain = parsed.domain.to_ascii_lowercase();
                full.push(parsed);
            }
        }
        Some(Self {
            domains: DnsConstraints::new(&domains, permitted),
            full,
        })
    }

    fn query(&self, s: &Mailbox) -> Option<String> {
        if self.full.contains(s) {
            return Some(s.text());
        }
        self.domains.as_ref()?.query(&s.domain)
    }
}

/// Go: `net.IP.To4`.
fn to4(ip: &[u8]) -> Option<&[u8]> {
    match ip.len() {
        4 => Some(ip),
        16 => {
            let (prefix, v4) = ip.split_at(12);
            (prefix == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF]).then_some(v4)
        }
        _ => None,
    }
}

impl IpNet {
    /// Go: `networkNumberAndMask`.
    fn network_and_mask(&self) -> Option<(&[u8], &[u8])> {
        let ip = match to4(&self.ip) {
            Some(v4) => v4,
            None if self.ip.len() == 16 => &self.ip,
            None => return None,
        };
        let mask = match self.mask.len() {
            4 if ip.len() == 4 => &self.mask,
            16 if ip.len() == 4 => self.mask.get(12..)?,
            16 => &self.mask,
            _ => return None,
        };
        Some((ip, mask))
    }

    /// Go: `IPNet.Contains`.
    fn contains(&self, ip: &[u8]) -> bool {
        let Some((network, mask)) = self.network_and_mask() else {
            return false;
        };
        let ip = to4(ip).unwrap_or(ip);
        ip.len() == network.len()
            && network
                .iter()
                .zip(mask)
                .zip(ip)
                .all(|((n, m), i)| n & m == i & m)
    }

    /// Go: `IPNet.String`.
    fn text(&self) -> String {
        let Some((network, mask)) = self.network_and_mask() else {
            return "<nil>".to_owned();
        };
        let ip = ip_text(network);
        let ones: u32 = mask.iter().map(|b| b.leading_ones()).sum();
        let canonical = mask.iter().enumerate().all(|(i, &b)| {
            let before = u32::try_from(i * 8).unwrap_or(u32::MAX);
            let want = ones.saturating_sub(before).min(8);
            u32::from(b) == (0xFF_u32 << (8 - want)) & 0xFF
        });
        if canonical {
            format!("{ip}/{ones}")
        } else {
            format!("{ip}/{}", hex(mask))
        }
    }
}

/// Go: `net.IP.String` of 4 or 16 bytes (an IPv4-mapped address prints as IPv4).
fn ip_text(ip: &[u8]) -> String {
    if let Ok(v4) = <[u8; 4]>::try_from(ip) {
        return IpAddr::from(v4).to_string();
    }
    if let Ok(v6) = <[u8; 16]>::try_from(ip) {
        let v6 = Ipv6Addr::from(v6);
        return v6
            .to_ipv4_mapped()
            .map_or_else(|| v6.to_string(), |v4| v4.to_string());
    }
    String::from("?")
}

/// Go: `ipNetworkSubset(a, b)`: whether `b` lies within `a`.
fn ip_network_subset(a: &IpNet, b: &IpNet) -> bool {
    if !a.contains(&b.ip) {
        return false;
    }
    let broadcast: Vec<u8> =
        b.ip.iter()
            .zip(b.mask.iter().chain(iter::repeat(&0)))
            .map(|(i, m)| i | !m)
            .collect();
    a.contains(&broadcast)
}

/// Go: `ipConstraints`: IPv4 and IPv6 ranges apart, by the length of the constraint's address.
#[derive(Debug)]
struct IpConstraints {
    v4: Vec<IpNet>,
    v6: Vec<IpNet>,
}

impl IpConstraints {
    /// Go: `newIPNetConstraints`.
    fn new(list: &[IpNet]) -> Option<Self> {
        if list.is_empty() {
            return None;
        }
        let (mut v4, mut v6): (Vec<IpNet>, Vec<IpNet>) =
            list.iter().cloned().partition(|n| n.ip.len() == 4);
        for set in [&mut v4, &mut v6] {
            sort_and_prune(
                set,
                |a, b| a.ip.cmp(&b.ip).then_with(|| a.mask.cmp(&b.mask)),
                ip_network_subset,
            );
        }
        Some(Self { v4, v6 })
    }

    /// Go: `ipConstraints.query`, by the length of the name's address.
    fn query(&self, ip: &[u8]) -> Option<String> {
        let set = if ip.len() == 4 { &self.v4 } else { &self.v6 };
        search(set, ip, |c, ip| c.ip.as_slice().cmp(ip), IpNet::contains).map(IpNet::text)
    }
}

/// Go: one `chainConstraints` link: a certificate's constraints.
#[derive(Debug)]
struct Constraints {
    index: usize,
    ip: (Option<IpConstraints>, Option<IpConstraints>),
    dns: (Option<DnsConstraints>, Option<DnsConstraints>),
    uri: (Option<DnsConstraints>, Option<DnsConstraints>),
    email: (Option<EmailConstraints>, Option<EmailConstraints>),
}

/// A name looked up in a certificate's permitted or excluded constraints of its type.
#[derive(Debug)]
enum Lookup {
    /// The certificate has none of the type (Go: a nil list), so the name passes.
    Unconstrained,
    /// The constraint that matched, if one did.
    Searched(Option<String>),
}

impl Lookup {
    fn of<C>(constraints: Option<&C>, query: impl FnOnce(&C) -> Option<String>) -> Self {
        constraints.map_or(Self::Unconstrained, |c| Self::Searched(query(c)))
    }
}

/// Lower-case hex, as Go's `%x` prints bytes.
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|&b| [b >> 4, b & 0x0F])
        .filter_map(|digit| char::from_digit(u32::from(digit), 16))
        .collect()
}

/// Go: `checkConstraints`'s texts.
fn verdict(kind: &str, name: &str, permitted: &Lookup, excluded: Lookup) -> Result<(), String> {
    if matches!(permitted, Lookup::Searched(None)) {
        return Err(format!(
            "{kind} {} is not permitted by any constraint",
            quote(name)
        ));
    }
    if let Lookup::Searched(Some(constraint)) = excluded {
        return Err(format!(
            "{kind} {} is excluded by constraint {}",
            quote(name),
            quote(&constraint)
        ));
    }
    Ok(())
}

/// A URI name prepared for matching (Go: `parsedURI`).
#[derive(Debug)]
struct ParsedUri<'a> {
    uri: &'a Url,
    domain: String,
}

impl Constraints {
    fn new(index: usize, permitted: &Subtrees, excluded: &Subtrees) -> Self {
        Self {
            index,
            ip: (
                IpConstraints::new(&permitted.ips),
                IpConstraints::new(&excluded.ips),
            ),
            dns: (
                DnsConstraints::new(&permitted.dns, true),
                DnsConstraints::new(&excluded.dns, false),
            ),
            uri: (
                DnsConstraints::new(&permitted.uris, true),
                DnsConstraints::new(&excluded.uris, false),
            ),
            email: (
                EmailConstraints::new(&permitted.emails, true),
                EmailConstraints::new(&excluded.emails, false),
            ),
        }
    }

    /// Go: `chainConstraints.check`.
    fn check(
        &self,
        dns: &[String],
        uris: &[ParsedUri<'_>],
        emails: &[Mailbox],
        ips: &[Vec<u8>],
    ) -> Result<(), String> {
        for ip in ips {
            verdict(
                "IP address",
                &ip_text(ip),
                &Lookup::of(self.ip.0.as_ref(), |c| c.query(ip)),
                Lookup::of(self.ip.1.as_ref(), |c| c.query(ip)),
            )?;
        }
        for d in dns {
            if !domain_name_valid(d, false) {
                return Err(format!("x509: cannot parse dnsName {}", quote(d)));
            }
            verdict(
                "DNS name",
                d,
                &Lookup::of(self.dns.0.as_ref(), |c| c.query(d)),
                Lookup::of(self.dns.1.as_ref(), |c| c.query(d)),
            )?;
        }
        for u in uris {
            if !domain_name_valid(&u.domain, false) {
                return Err(format!(
                    "x509: internal error: URI SAN {} failed to parse",
                    quote(&u.uri.to_string())
                ));
            }
            verdict(
                "URI",
                &u.uri.to_string(),
                &Lookup::of(self.uri.0.as_ref(), |c| c.query(&u.domain)),
                Lookup::of(self.uri.1.as_ref(), |c| c.query(&u.domain)),
            )?;
        }
        for e in emails {
            if !domain_name_valid(&e.domain, false) {
                return Err(format!(
                    "x509: cannot parse rfc822Name {}",
                    quote(&e.text())
                ));
            }
            verdict(
                "email address",
                &e.text(),
                &Lookup::of(self.email.0.as_ref(), |c| c.query(e)),
                Lookup::of(self.email.1.as_ref(), |c| c.query(e)),
            )?;
        }
        Ok(())
    }
}

/// Go: `net.SplitHostPort`'s host, or its error text.
fn split_host(hostport: &str) -> Result<&str, String> {
    let fail = |why: &str| Err(format!("address {hostport}: {why}"));
    let Some(colon) = hostport.rfind(':') else {
        return fail("missing port in address");
    };
    let (host, from, to) = if hostport.starts_with('[') {
        let Some(end) = hostport.find(']') else {
            return fail("missing ']' in address");
        };
        if end + 1 == hostport.len() {
            return fail("missing port in address");
        }
        if end + 1 != colon {
            return if hostport.as_bytes().get(end + 1) == Some(&b':') {
                fail("too many colons in address")
            } else {
                fail("missing port in address")
            };
        }
        (hostport.get(1..end).unwrap_or_default(), 1, end + 1)
    } else {
        let host = hostport.get(..colon).unwrap_or_default();
        if host.contains(':') {
            return fail("too many colons in address");
        }
        (host, 0, 0)
    };
    if hostport.get(from..).is_some_and(|s| s.contains('[')) {
        return fail("unexpected '[' in address");
    }
    if hostport.get(to..).is_some_and(|s| s.contains(']')) {
        return fail("unexpected ']' in address");
    }
    Ok(host)
}

/// Go: `parseURIs`.
fn parse_uris(uris: &[Url]) -> Result<Vec<ParsedUri<'_>>, String> {
    uris.iter()
        .map(|uri| {
            let mut host = uri.host.to_ascii_lowercase();
            if host.is_empty() {
                return Err(format!(
                    "URI with empty host ({}) cannot be matched against constraints",
                    quote(&uri.to_string())
                ));
            }
            if host.contains(':') && !host.ends_with(']') {
                // Go splits the URI's host as written, not the lower-cased one.
                split_host(&uri.host)
                    .map_err(|e| format!("cannot parse URI host {}: {e}", quote(&uri.host)))?
                    .clone_into(&mut host);
            }
            if host.parse::<IpAddr>().is_ok() || (host.starts_with('[') && host.ends_with(']')) {
                return Err(format!(
                    "URI with IP ({}) cannot be matched against constraints",
                    quote(&uri.to_string())
                ));
            }
            Ok(ParsedUri { uri, domain: host })
        })
        .collect()
}

/// Go: `parseMailboxes`.
fn parse_mailboxes(emails: &[String]) -> Result<Vec<Mailbox>, String> {
    emails
        .iter()
        .map(|email| {
            let mut mailbox = parse_mailbox(email)
                .ok_or_else(|| format!("cannot parse rfc822Name {}", quote(email)))?;
            mailbox.domain = mailbox.domain.to_ascii_lowercase();
            Ok(mailbox)
        })
        .collect()
}

/// Go: `checkChainConstraints`: the chain runs from the leaf to the root.
pub(super) fn check_chain(chain: &[&Certificate]) -> Result<(), String> {
    let links: Vec<Constraints> = chain
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            c.name_constraints
                .as_ref()
                .map(|nc| Constraints::new(i, &nc.permitted, &nc.excluded))
        })
        .collect();
    let Some(first) = links.first() else {
        return Ok(());
    };
    let mut current = 0_usize;
    let mut current_index = first.index;
    for (i, c) in chain.iter().enumerate() {
        if c.san.is_none() {
            continue;
        }
        if i >= current_index {
            while current_index <= i {
                current += 1;
                match links.get(current) {
                    Some(next) => current_index = next.index,
                    None => return Ok(()),
                }
            }
        }
        let uris = parse_uris(&c.uris)?;
        let emails = parse_mailboxes(&c.email_addresses)?;
        let ips: Vec<Vec<u8>> = c
            .ip_addresses
            .iter()
            .map(|ip| match ip {
                IpAddr::V4(v4) => v4.octets().to_vec(),
                IpAddr::V6(v6) => v6.octets().to_vec(),
            })
            .collect();
        for link in links.get(current..).unwrap_or_default() {
            link.check(&c.dns_names, &uris, &emails, &ips)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    #[test]
    fn mailboxes() -> Result<(), String> {
        let parsed = |s: &str| parse_mailbox(s).map(|m| m.text());
        ensure_eq(
            &parsed("a.b@example.com"),
            &Some("a.b@example.com".to_owned()),
            "dot",
        )?;
        ensure_eq(&parsed("\"a b\"@x"), &Some("a b@x".to_owned()), "quoted")?;
        ensure_eq(&parsed("a..b@x"), &None, "two dots")?;
        ensure_eq(&parsed(".a@x"), &None, "leading dot")?;
        ensure_eq(&parsed("a@"), &Some("a@".to_owned()), "empty domain")?;
        ensure_eq(&parsed("a@b..c"), &None, "empty label")?;
        ensure_eq(&parsed("@@"), &None, "no local part")
    }

    #[test]
    fn dns_order_and_suffixes() -> Result<(), String> {
        ensure_eq(
            &dns_has_suffix("example.com", "a.Example.COM"),
            &true,
            "label",
        )?;
        ensure_eq(
            &dns_has_suffix("example.com", "aexample.com"),
            &false,
            "not a label",
        )?;
        ensure_eq(
            &dns_has_suffix(".example.com", "example.com"),
            &false,
            "dot",
        )?;
        ensure_eq(
            &dns_compare("example.com", "a.example.com"),
            &Ordering::Less,
            "parent first",
        )?;
        let c = DnsConstraints::new(&["a.example.com".to_owned()], false);
        ensure_eq(
            &c.and_then(|c| c.query("*.example.com")),
            &Some("a.example.com".to_owned()),
            "wildcard against an excluded child",
        )
    }

    #[test]
    fn host_splits() -> Result<(), String> {
        ensure_eq(&split_host("example.com:80"), &Ok("example.com"), "port")?;
        ensure_eq(
            &split_host("a:b:c"),
            &Err("address a:b:c: too many colons in address".to_owned()),
            "colons",
        )?;
        ensure_eq(&split_host("[::1]:80"), &Ok("::1"), "brackets")
    }

    #[test]
    fn ip_texts() -> Result<(), String> {
        let net = IpNet {
            ip: vec![10, 1, 2, 3],
            mask: vec![255, 0, 0, 0],
        };
        ensure_eq(net.text().as_str(), "10.1.2.3/8", "unmasked address")?;
        let odd = IpNet {
            ip: vec![10, 0, 0, 0],
            mask: vec![255, 0, 255, 0],
        };
        ensure_eq(odd.text().as_str(), "10.0.0.0/ff00ff00", "mask in hex")?;
        ensure_eq(&net.contains(&[10, 9, 9, 9]), &true, "contains")
    }
}
