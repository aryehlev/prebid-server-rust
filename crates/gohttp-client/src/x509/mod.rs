//! Go `crypto/x509` server certificate verification, as `crypto/tls`'s client runs it
//! (`Conn.verifyServerCertificate`: `Certificate.Verify` with the root pool, the presented
//! intermediates and `Config.ServerName`).
//!
//! Go's order is kept, so a certificate that fails several checks gets Go's error: every
//! presented certificate is parsed first (Go's `ParseCertificate`, with its error texts); then the
//! leaf's critical extensions and validity are checked, then its names (`VerifyHostname`, with
//! the legacy common name text), and a leaf that is itself in the root pool is a chain of one
//! (Go: `CertPool.contains`). Only then is a path built: rustls-webpki builds it (signatures,
//! the intermediates' validity, basic constraints and path lengths, name constraints and its
//! distrust list) with every extended key usage accepted, and each path it finds is checked the
//! way Go's `buildChains` and `Verify` check theirs, for what webpki leaves out: each issuer's
//! key usage (`keyCertSign`), the root's basic constraints, critical extensions, validity and
//! path length, and the extended key usages nested down the chain (`checkChainForKeyUsage` with
//! `serverAuth`). When no path verifies, the error is Go's for the paths webpki found, one
//! rejected only for its extended key usages first (Go checks those on the chains that pass
//! everything else); without such paths, webpki's error is mapped to Go's text.
//!
//! The differences from Go are listed in the [crate docs](crate#differences-from-go).

mod cert;
#[cfg(test)]
mod tests;

use std::{borrow::Cow, cell::Cell, error::Error as StdError, fmt, net::IpAddr, ptr, sync::Arc};

use pbs_gocompat::{
    strconv::quote,
    time::{self, Time},
};
use rustls::{
    CertificateError, DigitallySignedStruct, Error as TlsError, OtherError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, TrustAnchor, UnixTime},
};
use webpki::{
    EndEntityCert, Error as WebpkiError, ExtendedKeyUsageValidator, KeyPurposeIdIter, VerifiedPath,
};

use self::cert::{Certificate, KEY_USAGE_CERT_SIGN};

const UNKNOWN_AUTHORITY: &str = "x509: certificate signed by unknown authority";
const EXPIRED: &str = "x509: certificate has expired or is not yet valid: ";
const INCOMPATIBLE_USAGE: &str = "x509: certificate specifies an incompatible key usage";
const UNHANDLED_CRITICAL: &str = "x509: unhandled critical extension";
const TOO_MANY_INTERMEDIATES: &str = "x509: too many intermediates for path length constraint";
const NOT_AUTHORIZED_TO_SIGN: &str =
    "x509: certificate is not authorized to sign other certificates";
/// Go: `ConstraintViolationError`.
const CONSTRAINT_VIOLATION: &str =
    "x509: invalid signature: parent certificate cannot sign this kind of certificate";
const SIGNATURE_LIMIT: &str =
    "x509: signature check attempts limit reached while verifying certificate chain";
const LEGACY_COMMON_NAME: &str =
    "x509: certificate relies on legacy Common Name field, use SANs instead";
/// Go: `HostnameError`'s `maxNamesIncluded`.
const MAX_NAMES_INCLUDED: usize = 100;

/// Go's error for a server certificate the TLS client rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rejection {
    /// Go: `tls: failed to parse certificate from server: ...`.
    Parse(String),
    /// Go: `*tls.CertificateVerificationError`, `tls: failed to verify certificate: ...`.
    Verify(String),
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "tls: failed to parse certificate from server: {e}"),
            Self::Verify(e) => write!(f, "tls: failed to verify certificate: {e}"),
        }
    }
}

impl StdError for Rejection {}

/// Whether Go's `x509.ParseCertificate` accepts the certificate (`AppendCertsFromPEM` skips
/// the others).
pub(crate) fn parses(der: &[u8]) -> bool {
    cert::parse(der).is_ok()
}

/// A root pool ready for verification (Go: `VerifyOptions.Roots`).
#[derive(Debug)]
pub(crate) struct Roots {
    /// The pool's certificates that Go parses, with their fields.
    certs: Vec<(CertificateDer<'static>, Certificate)>,
    /// The certificates webpki can use as trust anchors (webpki takes version 1 roots and ignores
    /// the critical extensions it does not know in a root, so these are all of `certs` for the
    /// Mozilla bundle and macOS's `/etc/ssl/cert.pem`). Another root still counts for Go's
    /// pooled-leaf rule but anchors no path...
    anchors: Vec<TrustAnchor<'static>>,
    /// ...and the index in `certs` of each.
    anchor_certs: Vec<usize>,
    algs: WebPkiSupportedAlgorithms,
}

/// The certificates the server presented, raw and parsed.
#[derive(Debug, Clone, Copy)]
struct Presented<'a> {
    leaf: &'a Certificate,
    /// The intermediates, in the order the server sent them.
    ders: &'a [CertificateDer<'a>],
    certs: &'a [Certificate],
}

impl Presented<'_> {
    /// Go's candidate for a hint when webpki finds an intermediate that is not a CA: the first
    /// presented certificate without `cA` that issued the leaf or another presented
    /// certificate.
    fn non_ca_issuer(&self) -> Option<&Certificate> {
        self.certs.iter().find(|c| {
            !c.basic_constraints.is_some_and(|bc| bc.is_ca)
                && (self.leaf.issuer == c.subject
                    || self.certs.iter().any(|other| other.issuer == c.subject))
        })
    }
}

/// Accepts every extended key usage: Go checks them down the whole chain afterwards.
#[derive(Debug, Clone, Copy)]
struct AnyUsage;

impl ExtendedKeyUsageValidator for AnyUsage {
    fn validate(&self, _iter: KeyPurposeIdIter<'_, '_>) -> Result<(), WebpkiError> {
        Ok(())
    }
}

impl Roots {
    /// Parses the pool once (Go parses each pooled certificate lazily, with the same result).
    pub(crate) fn new(pool: &[CertificateDer<'static>], algs: WebPkiSupportedAlgorithms) -> Self {
        let mut certs = Vec::with_capacity(pool.len());
        let mut anchors = Vec::with_capacity(pool.len());
        let mut anchor_certs = Vec::with_capacity(pool.len());
        for der in pool {
            let Ok(parsed) = cert::parse(der) else {
                continue;
            };
            if let Ok(anchor) = webpki::anchor_from_trusted_cert(der) {
                anchors.push(anchor.to_owned());
                anchor_certs.push(certs.len());
            }
            certs.push((der.clone(), parsed));
        }
        Self {
            certs,
            anchors,
            anchor_certs,
            algs,
        }
    }

    /// Go: `verifyServerCertificate`'s parsing and `Certificate.Verify` for `host` at `now`.
    pub(crate) fn verify(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        host: &str,
        now: UnixTime,
    ) -> Result<(), Rejection> {
        let leaf = cert::parse(end_entity).map_err(Rejection::Parse)?;
        let certs = intermediates
            .iter()
            .map(|der| cert::parse(der))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Rejection::Parse)?;
        let go_now = Time::from_unix(i64::try_from(now.as_secs()).unwrap_or(i64::MAX), 0);
        check_valid(&leaf, &go_now).map_err(Rejection::Verify)?;
        verify_hostname(&leaf, host).map_err(Rejection::Verify)?;
        // Go: `opts.Roots.contains(c)`: a pooled leaf is a chain of one.
        if self
            .certs
            .iter()
            .any(|(der, _)| der.as_ref() == end_entity.as_ref())
        {
            return if leaf.server_auth {
                Ok(())
            } else {
                Err(Rejection::Verify(INCOMPATIBLE_USAGE.to_owned()))
            };
        }
        let presented = Presented {
            leaf: &leaf,
            ders: intermediates,
            certs: &certs,
        };
        self.build(end_entity, presented, now, &go_now)
            .map_err(Rejection::Verify)
    }

    /// Go: `buildChains` and `Verify`'s chain checks, over the paths webpki builds.
    fn build(
        &self,
        end_entity: &CertificateDer<'_>,
        presented: Presented<'_>,
        now: UnixTime,
        go_now: &Time,
    ) -> Result<(), String> {
        let ee =
            EndEntityCert::try_from(end_entity).map_err(|e| webpki_text(&e, presented, go_now))?;
        // Go's error for the paths webpki found and Go rejects. Go checks the extended key usages
        // only on the chains that pass every other check, so a path rejected for them decides.
        let rejected: Cell<Option<String>> = Cell::new(None);
        let check = |path: &VerifiedPath<'_>| {
            self.check_path(path, presented, go_now).map_err(|text| {
                let usage = rejected.take().filter(|e| e == INCOMPATIBLE_USAGE);
                rejected.set(Some(usage.unwrap_or(text)));
                WebpkiError::UnknownIssuer
            })
        };
        let verified = ee.verify_for_usage(
            self.algs.all,
            &self.anchors,
            presented.ders,
            now,
            AnyUsage,
            None,
            Some(&check),
        );
        match verified {
            Ok(_path) => Ok(()),
            Err(e) => Err(rejected
                .take()
                .unwrap_or_else(|| webpki_text(&e, presented, go_now))),
        }
    }

    /// Go's checks of a path that webpki does not make: `CheckSignatureFrom`'s checks of each
    /// issuer and `isValid`'s critical extensions, from the leaf up, then the root's validity
    /// and path length, then the extended key usages down the chain.
    fn check_path(
        &self,
        path: &VerifiedPath<'_>,
        presented: Presented<'_>,
        now: &Time,
    ) -> Result<(), String> {
        let root = self
            .anchors
            .iter()
            .position(|anchor| ptr::eq(anchor, path.anchor()))
            .and_then(|i| self.anchor_certs.get(i))
            .and_then(|&i| self.certs.get(i))
            .map(|(_, cert)| cert)
            .ok_or(UNKNOWN_AUTHORITY)?;
        let mut issuers: Vec<&Certificate> = Vec::new();
        for intermediate in path.intermediate_certificates() {
            let der = intermediate.der();
            let cert = presented
                .ders
                .iter()
                .position(|d| d.as_ref() == der.as_ref())
                .and_then(|i| presented.certs.get(i))
                .ok_or(UNKNOWN_AUTHORITY)?;
            issuers.push(cert);
        }
        let intermediates = issuers.len();
        issuers.push(root);
        for issuer in &issuers {
            if !may_sign(issuer) {
                return Err(hint(CONSTRAINT_VIOLATION, issuer));
            }
            if issuer.unhandled_critical {
                return Err(UNHANDLED_CRITICAL.to_owned());
            }
        }
        check_valid(root, now)?;
        let max_path_len = root.basic_constraints.and_then(|bc| bc.max_path_len);
        if max_path_len.is_some_and(|max| u64::try_from(intermediates).is_ok_and(|n| n > max)) {
            return Err(TOO_MANY_INTERMEDIATES.to_owned());
        }
        if !(presented.leaf.server_auth && issuers.iter().all(|c| c.server_auth)) {
            return Err(INCOMPATIBLE_USAGE.to_owned());
        }
        Ok(())
    }
}

/// Go: `CheckSignatureFrom`'s checks of the parent: a version 3 certificate needs basic
/// constraints, basic constraints need `cA`, and a key usage needs `keyCertSign`.
const fn may_sign(parent: &Certificate) -> bool {
    let ca = match parent.basic_constraints {
        Some(bc) => bc.is_ca,
        None => parent.version != 3,
    };
    ca && (parent.key_usage == 0 || parent.key_usage & KEY_USAGE_CERT_SIGN != 0)
}

/// Go: `UnknownAuthorityError` with the error that rejected a candidate issuer.
fn hint(reason: &str, candidate: &Certificate) -> String {
    format!(
        "{UNKNOWN_AUTHORITY} (possibly because of {} while trying to verify candidate authority \
         certificate {})",
        quote(reason),
        quote(&candidate.display_name())
    )
}

/// Go: `isValid`'s checks that do not depend on the chain: critical extensions, then validity.
fn check_valid(cert: &Certificate, now: &Time) -> Result<(), String> {
    if cert.unhandled_critical {
        return Err(UNHANDLED_CRITICAL.to_owned());
    }
    if now.before(&cert.not_before) {
        return Err(not_yet_valid(now, &cert.not_before));
    }
    if now.after(&cert.not_after) {
        return Err(expired(now, &cert.not_after));
    }
    Ok(())
}

fn not_yet_valid(now: &Time, not_before: &Time) -> String {
    format!(
        "{EXPIRED}current time {} is before {}",
        now.format(time::RFC3339),
        not_before.format(time::RFC3339)
    )
}

fn expired(now: &Time, not_after: &Time) -> String {
    format!(
        "{EXPIRED}current time {} is after {}",
        now.format(time::RFC3339),
        not_after.format(time::RFC3339)
    )
}

/// A webpki time as a Go time.
fn go_time(t: UnixTime) -> Time {
    Time::from_unix(i64::try_from(t.as_secs()).unwrap_or(i64::MAX), 0)
}

/// Go's text for the failure webpki reports when no path verified.
fn webpki_text(e: &WebpkiError, presented: Presented<'_>, now: &Time) -> String {
    if let WebpkiError::CertExpired { not_after, .. } = e {
        return expired(now, &go_time(*not_after));
    }
    if let WebpkiError::CertNotValidYet { not_before, .. } = e {
        return not_yet_valid(now, &go_time(*not_before));
    }
    if matches!(e, WebpkiError::UnsupportedCriticalExtension) {
        return UNHANDLED_CRITICAL.to_owned();
    }
    if matches!(e, WebpkiError::PathLenConstraintViolated) {
        return TOO_MANY_INTERMEDIATES.to_owned();
    }
    if matches!(e, WebpkiError::MaximumSignatureChecksExceeded) {
        return SIGNATURE_LIMIT.to_owned();
    }
    if matches!(
        e,
        WebpkiError::EndEntityUsedAsCa | WebpkiError::UnsupportedCertVersion
    ) && let Some(candidate) = presented.non_ca_issuer()
    {
        // Go rejects a version 3 candidate without cA in CheckSignatureFrom (a hint), and an
        // older one, which webpki does not parse, in isValid.
        return if may_sign(candidate) {
            NOT_AUTHORIZED_TO_SIGN.to_owned()
        } else {
            hint(CONSTRAINT_VIOLATION, candidate)
        };
    }
    if matches!(
        e,
        WebpkiError::UnknownIssuer
            | WebpkiError::InvalidSignatureForPublicKey
            | WebpkiError::UnsupportedSignatureAlgorithmContext(_)
            | WebpkiError::UnsupportedSignatureAlgorithmForPublicKeyContext(_)
            | WebpkiError::SignatureAlgorithmMismatch
            | WebpkiError::CaUsedAsEndEntity
            | WebpkiError::EndEntityUsedAsCa
            | WebpkiError::UnsupportedCertVersion
            | WebpkiError::CertDistrusted
            | WebpkiError::MaximumPathDepthExceeded
            | WebpkiError::MaximumPathBuildCallsExceeded
    ) {
        return UNKNOWN_AUTHORITY.to_owned();
    }
    format!("x509: {e}")
}

/// Go: `Certificate.VerifyHostname`.
fn verify_hostname(cert: &Certificate, h: &str) -> Result<(), String> {
    let candidate_ip = if h.len() >= 3 {
        h.strip_prefix('[').and_then(|rest| rest.strip_suffix(']'))
    } else {
        None
    }
    .unwrap_or(h);
    if let Ok(ip) = candidate_ip.parse::<IpAddr>() {
        if cert
            .ip_addresses
            .iter()
            .any(|san| san.to_canonical() == ip.to_canonical())
        {
            return Ok(());
        }
        return Err(hostname_error(cert, candidate_ip));
    }
    let name = h.to_ascii_lowercase();
    let valid_name = valid_hostname(&name, false);
    let matched = cert.dns_names.iter().any(|pattern| {
        if valid_name && valid_hostname(pattern, true) {
            match_hostnames(pattern, &name)
        } else {
            match_exactly(pattern, &name)
        }
    });
    if matched {
        Ok(())
    } else {
        Err(hostname_error(cert, h))
    }
}

/// Go: `HostnameError.Error`.
fn hostname_error(cert: &Certificate, host: &str) -> String {
    if !cert.has_san && match_hostnames(&cert.common_name, host) {
        return LEGACY_COMMON_NAME.to_owned();
    }
    let valid = if host.parse::<IpAddr>().is_ok() {
        if cert.ip_addresses.is_empty() {
            return format!(
                "x509: cannot validate certificate for {host} because it doesn't contain any IP \
                 SANs"
            );
        }
        if cert.ip_addresses.len() >= MAX_NAMES_INCLUDED {
            return format!(
                "x509: certificate is valid for {} IP SANs, but none matched {host}",
                cert.ip_addresses.len()
            );
        }
        cert.ip_addresses
            .iter()
            .map(go_ip_text)
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        if cert.dns_names.len() >= MAX_NAMES_INCLUDED {
            return format!(
                "x509: certificate is valid for {} names, but none matched {host}",
                cert.dns_names.len()
            );
        }
        cert.dns_names.join(", ")
    };
    if valid.is_empty() {
        format!("x509: certificate is not valid for any names, but wanted to match {host}")
    } else {
        format!("x509: certificate is valid for {valid}, not {host}")
    }
}

/// Go: `net.IP.String` (an IPv4-mapped address prints as IPv4).
fn go_ip_text(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or_else(|| v6.to_string(), |v4| v4.to_string()),
    }
}

/// Go: `validHostname`.
fn valid_hostname(host: &str, is_pattern: bool) -> bool {
    let host = if is_pattern {
        host
    } else {
        host.strip_suffix('.').unwrap_or(host)
    };
    if host.is_empty() || host == "*" {
        return false;
    }
    host.split('.').enumerate().all(|(i, part)| {
        if part.is_empty() {
            return false;
        }
        if is_pattern && i == 0 && part == "*" {
            return true;
        }
        part.chars()
            .enumerate()
            .all(|(j, c)| c.is_ascii_alphanumeric() || (c == '-' && j != 0) || c == '_')
    })
}

/// Go: `matchExactly`.
fn match_exactly(a: &str, b: &str) -> bool {
    !(a.is_empty() || a == "." || b.is_empty() || b == ".") && a.eq_ignore_ascii_case(b)
}

/// Go: `matchHostnames`: a wildcard matches one whole left-most label.
fn match_hostnames(pattern: &str, host: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if pattern.is_empty() || host.is_empty() {
        return false;
    }
    let pattern_parts: Vec<&str> = pattern.split('.').collect();
    let host_parts: Vec<&str> = host.split('.').collect();
    pattern_parts.len() == host_parts.len()
        && pattern_parts
            .iter()
            .zip(&host_parts)
            .enumerate()
            .all(|(i, (p, h))| (i == 0 && *p == "*") || p == h)
}

/// The rustls verifier for a handshake: Go's checks against `host` (Go: `Config.ServerName`,
/// or the dialed host).
#[derive(Debug)]
pub(crate) struct ServerVerifier {
    roots: Arc<Roots>,
    /// The name as the transport has it; rustls's [`ServerName`] drops a trailing dot.
    host: Option<String>,
}

impl ServerVerifier {
    pub(crate) const fn new(roots: Arc<Roots>, host: Option<String>) -> Self {
        Self { roots, host }
    }
}

impl ServerCertVerifier for ServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let host: Cow<'_, str> = self
            .host
            .as_deref()
            .map_or_else(|| server_name.to_str(), Cow::Borrowed);
        self.roots
            .verify(end_entity, intermediates, &host, now)
            .map(|()| ServerCertVerified::assertion())
            .map_err(|r| {
                TlsError::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(r))))
            })
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, cert, dss, &self.roots.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, cert, dss, &self.roots.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.roots.algs.supported_schemes()
    }
}
