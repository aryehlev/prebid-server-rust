//! Go `crypto/x509` server certificate verification, as `crypto/tls`'s client runs it.
//!
//! The client's `Conn.verifyServerCertificate` parses the presented certificates, runs
//! `Certificate.Verify` with the root pool, the presented intermediates and `Config.ServerName`,
//! then checks the leaf's key.
//!
//! This is a port of Go 1.26's verifier, not a wrapper around another one: a chain verifies when
//! Go's verifier accepts it, and a rejected one gets Go's error, but for the differences the
//! [crate docs](crate#differences-from-go) list.
//!
//! - Every presented certificate is parsed first, with Go's rules and texts (the `cert` module),
//!   and an RSA key over 8192 bits is refused.
//! - The leaf's critical extensions and validity are checked, then its names (`VerifyHostname`).
//! - A leaf in the root pool is a chain of one; otherwise chains are built as `buildChains` builds
//!   them: candidate issuers by name, ordered by key identifiers, roots before intermediates, Go's
//!   loop check, its limit of 100 signature checks and its error for a failed search (the last
//!   candidate error, or the unknown authority error with the first candidate error as its hint).
//! - Signatures are checked as `checkSignature` checks them, with Go's texts, and verified with
//!   rustls's aws-lc-rs algorithms.
//! - Each chain is then checked for its policies (the `policy` module), extended key usages and
//!   name constraints (the `constraints` module), with Go's error when none passes.

mod cert;
mod constraints;
mod ec;
mod policy;
#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, error::Error as StdError, fmt, net::IpAddr, slice, sync::Arc};

use pbs_gocompat::{
    strconv::quote,
    time::{self, Time},
};
use rustls::{
    CertificateError, DigitallySignedStruct, Error as TlsError, OtherError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{AlgorithmIdentifier, CertificateDer, ServerName, UnixTime, alg_id},
};

use self::{
    cert::{Certificate, KEY_USAGE_CERT_SIGN, PublicKey, SignatureAlgorithm},
    ec::Curve,
};

const UNKNOWN_AUTHORITY: &str = "x509: certificate signed by unknown authority";
const EXPIRED: &str = "x509: certificate has expired or is not yet valid: ";
const INCOMPATIBLE_USAGE: &str = "x509: certificate specifies an incompatible key usage";
const UNHANDLED_CRITICAL: &str = "x509: unhandled critical extension";
const TOO_MANY_INTERMEDIATES: &str = "x509: too many intermediates for path length constraint";
const NOT_AUTHORIZED_TO_SIGN: &str =
    "x509: certificate is not authorized to sign other certificates";
/// Go: `CANotAuthorizedForThisName`.
const NOT_AUTHORIZED_FOR_NAME: &str =
    "x509: a root or intermediate certificate is not authorized to sign for this name: ";
/// Go: `ConstraintViolationError`.
const CONSTRAINT_VIOLATION: &str =
    "x509: invalid signature: parent certificate cannot sign this kind of certificate";
/// Go: `ErrUnsupportedAlgorithm`.
const UNSUPPORTED_ALGORITHM: &str = "x509: cannot verify signature: algorithm unimplemented";
/// Go: `errSignatureLimit`.
const SIGNATURE_LIMIT: &str =
    "x509: signature check attempts limit reached while verifying certificate chain";
const LEGACY_COMMON_NAME: &str =
    "x509: certificate relies on legacy Common Name field, use SANs instead";
/// Go: `HostnameError`'s `maxNamesIncluded`.
const MAX_NAMES_INCLUDED: usize = 100;
/// Go: `maxChainSignatureChecks`.
const MAX_SIGNATURE_CHECKS: usize = 100;
/// Go: `crypto/tls`'s `checkKeySize` limit for a presented RSA key (`tlsmaxrsasize` unset).
const MAX_RSA_BITS: usize = 8192;

/// Go's error for a server certificate the TLS client rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rejection {
    /// Go: `tls: failed to parse certificate from server: ...`.
    Parse(String),
    /// Go: `*tls.CertificateVerificationError`, `tls: failed to verify certificate: ...`.
    Verify(String),
    /// A `crypto/tls` error about a presented key, as Go words it.
    Key(String),
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "tls: failed to parse certificate from server: {e}"),
            Self::Verify(e) => write!(f, "tls: failed to verify certificate: {e}"),
            Self::Key(e) => f.write_str(e),
        }
    }
}

impl StdError for Rejection {}

/// Whether Go's `x509.ParseCertificate` accepts the certificate (`AppendCertsFromPEM` skips
/// the others).
pub(crate) fn parses(der: &[u8]) -> bool {
    cert::parse(der).is_ok()
}

/// Go: `CertPool`: certificates in the order they were added, each once (by its bytes), found by
/// subject.
#[derive(Debug, Default)]
struct Pool {
    certs: Vec<Certificate>,
    /// Go: `byName`: the indices of the certificates with each `RawSubject`.
    by_name: BTreeMap<Vec<u8>, Vec<usize>>,
}

impl Pool {
    /// Go: `AddCert`.
    fn add(&mut self, cert: Certificate) {
        if self.contains(&cert.raw) {
            return;
        }
        self.by_name
            .entry(cert.subject.clone())
            .or_default()
            .push(self.certs.len());
        self.certs.push(cert);
    }

    /// Go: `CertPool.contains`.
    fn contains(&self, raw: &[u8]) -> bool {
        self.certs.iter().any(|c| c.raw == raw)
    }

    /// Go: `findPotentialParents`: the certificates whose subject is `child`'s issuer, those
    /// whose subject key identifier is `child`'s authority key identifier first, then those
    /// where only one of the two is present, then the rest.
    fn potential_parents(&self, child: &Certificate) -> Vec<&Certificate> {
        let mut matching = Vec::new();
        let mut one = Vec::new();
        let mut mismatch = Vec::new();
        let indices = self
            .by_name
            .get(&child.issuer)
            .map_or(&[][..], Vec::as_slice);
        for cert in indices.iter().filter_map(|&i| self.certs.get(i)) {
            if cert.subject_key_id == child.authority_key_id {
                matching.push(cert);
            } else if cert.subject_key_id.is_empty() != child.authority_key_id.is_empty() {
                one.push(cert);
            } else {
                mismatch.push(cert);
            }
        }
        matching.extend(one);
        matching.extend(mismatch);
        matching
    }
}

/// A root pool ready for verification (Go: `VerifyOptions.Roots`).
#[derive(Debug)]
pub(crate) struct Roots {
    /// The pool's certificates that Go parses.
    pool: Pool,
    /// Why the system pool did not load, when this is the system pool (Go: a nil
    /// `systemRootsPool()`, whose `SystemRootsError` every verification reports).
    system_error: Option<String>,
    algs: WebPkiSupportedAlgorithms,
}

/// Go: `leafCertificate`, `intermediateCertificate` and `rootCertificate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Leaf,
    Intermediate,
    Root,
}

/// Why `buildChains` built no chain.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BuildError {
    /// Go: `errSignatureLimit`, which stops the search.
    SignatureLimit,
    /// Any other error, as Go's text.
    Other(String),
}

impl BuildError {
    fn into_text(self) -> String {
        match self {
            Self::SignatureLimit => SIGNATURE_LIMIT.to_owned(),
            Self::Other(e) => e,
        }
    }
}

/// Go: one `Verify` call's `buildChains` recursion.
#[derive(Debug)]
struct Builder<'a> {
    roots: &'a Pool,
    intermediates: &'a Pool,
    algs: &'a WebPkiSupportedAlgorithms,
    now: &'a Time,
    /// Go: `sigChecks`, shared by the whole search.
    sig_checks: usize,
}

impl<'a> Builder<'a> {
    /// Go: `buildChains(currentChain)`: every chain from the last certificate of `current` to a
    /// root, or Go's error. Go's named result `err` is overwritten by each candidate that passes
    /// the signature check, so the error of a search that finds nothing is the last such
    /// candidate's (it is invalid, or no chain was found above it), and the unknown authority
    /// error only when none passed.
    fn build(
        &mut self,
        current: &[&'a Certificate],
    ) -> Result<Vec<Vec<&'a Certificate>>, BuildError> {
        let Some(&child) = current.last() else {
            return Ok(Vec::new());
        };
        let (roots, intermediates) = (self.roots, self.intermediates);
        let mut chains = Vec::new();
        let mut err: Option<BuildError> = None;
        let mut hint: Option<(String, &Certificate)> = None;
        'candidates: for (kind, parents) in [
            (Kind::Root, roots.potential_parents(child)),
            (Kind::Intermediate, intermediates.potential_parents(child)),
        ] {
            for candidate in parents {
                self.sig_checks += 1;
                if self.sig_checks > MAX_SIGNATURE_CHECKS {
                    err = Some(BuildError::SignatureLimit);
                    break 'candidates;
                }
                if candidate.public_key == PublicKey::Unknown
                    || already_in_chain(candidate, current)
                {
                    continue;
                }
                if let Err(e) = check_signature_from(child, candidate, self.algs) {
                    if hint.is_none() {
                        hint = Some((e, candidate));
                    }
                    continue;
                }
                if let Err(e) = is_valid(candidate, kind, current, self.now) {
                    if hint.is_none() {
                        hint = Some((e.clone(), candidate));
                    }
                    err = Some(BuildError::Other(e));
                    continue;
                }
                err = None;
                let chain = [current, slice::from_ref(&candidate)].concat();
                if kind == Kind::Root {
                    chains.push(chain);
                    continue;
                }
                match self.build(&chain) {
                    Ok(found) => chains.extend(found),
                    Err(BuildError::SignatureLimit) => {
                        err = Some(BuildError::SignatureLimit);
                        break 'candidates;
                    }
                    Err(e) => err = Some(e),
                }
            }
        }
        if !chains.is_empty() {
            return Ok(chains);
        }
        Err(err.unwrap_or_else(|| BuildError::Other(unknown_authority(hint))))
    }
}

impl Roots {
    /// Parses the pool once (Go parses each pooled certificate lazily, with the same result).
    /// `system_error` is why the system pool did not load, when the pool stands for it.
    pub(crate) fn new(
        pool: &[CertificateDer<'static>],
        system_error: Option<String>,
        algs: WebPkiSupportedAlgorithms,
    ) -> Self {
        let mut parsed = Pool::default();
        for cert in pool.iter().filter_map(|der| cert::parse(der).ok()) {
            parsed.add(cert);
        }
        Self {
            pool: parsed,
            system_error,
            algs,
        }
    }

    /// Go: `verifyServerCertificate` for `host` at `now`: the presented certificates are parsed,
    /// `Certificate.Verify` runs, and the leaf's key must be one TLS signs with.
    pub(crate) fn verify(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        host: &str,
        now: &Time,
    ) -> Result<(), Rejection> {
        let leaf = parse_presented(end_entity)?;
        let mut presented = Pool::default();
        for der in intermediates {
            presented.add(parse_presented(der)?);
        }
        if let Some(e) = &self.system_error {
            return Err(Rejection::Verify(format!(
                "x509: failed to load system roots and no roots provided; {e}"
            )));
        }
        self.verify_chains(&leaf, &presented, host, now)
            .map_err(Rejection::Verify)?;
        let key_type = match leaf.public_key {
            PublicKey::Rsa | PublicKey::Ecdsa(_) | PublicKey::Ed25519 => return Ok(()),
            PublicKey::Dsa => "*dsa.PublicKey",
            PublicKey::Unknown => "<nil>",
        };
        Err(Rejection::Key(format!(
            "tls: server's certificate contains an unsupported type of public key: {key_type}"
        )))
    }

    /// Go: `Certificate.Verify` from the leaf's checks on.
    fn verify_chains(
        &self,
        leaf: &Certificate,
        intermediates: &Pool,
        host: &str,
        now: &Time,
    ) -> Result<(), String> {
        is_valid(leaf, Kind::Leaf, &[], now)?;
        if !host.is_empty() {
            verify_hostname(leaf, host)?;
        }
        let candidates = if self.pool.contains(&leaf.raw) {
            vec![vec![leaf]]
        } else {
            let mut builder = Builder {
                roots: &self.pool,
                intermediates,
                algs: &self.algs,
                now,
                sig_checks: 0,
            };
            builder.build(&[leaf]).map_err(BuildError::into_text)?
        };
        let mut invalid_policies = 0_usize;
        let mut incompatible_usage = 0_usize;
        let mut constraints_error: Option<String> = None;
        for chain in &candidates {
            if !policy::policies_valid(chain) {
                invalid_policies += 1;
            } else if !chain.iter().all(|c| c.server_auth) {
                // Go: checkChainForKeyUsage with ExtKeyUsageServerAuth.
                incompatible_usage += 1;
            } else if let Err(e) = constraints::check_chain(chain) {
                if constraints_error.is_none() {
                    constraints_error = Some(format!("{NOT_AUTHORIZED_FOR_NAME}{e}"));
                }
            } else {
                return Ok(());
            }
        }
        if let Some(e) = constraints_error {
            return Err(e);
        }
        if incompatible_usage > 0 && invalid_policies == 0 {
            return Err(INCOMPATIBLE_USAGE.to_owned());
        }
        let mut details = Vec::new();
        if incompatible_usage > 0 {
            details.push(format!(
                "{incompatible_usage} candidate chains with incompatible key usage"
            ));
        }
        if invalid_policies > 0 {
            details.push(format!(
                "{invalid_policies} candidate chains with invalid policies"
            ));
        }
        Err(if details.is_empty() {
            "x509: no valid chains built".to_owned()
        } else {
            format!("x509: no valid chains built: {}", details.join(", "))
        })
    }
}

/// Go: one certificate of `verifyServerCertificate`'s parsing loop.
fn parse_presented(der: &[u8]) -> Result<Certificate, Rejection> {
    let cert = cert::parse(der).map_err(Rejection::Parse)?;
    if cert.rsa.is_some_and(|key| key.bits > MAX_RSA_BITS) {
        return Err(Rejection::Key(format!(
            "tls: server sent certificate containing RSA key larger than {MAX_RSA_BITS} bits"
        )));
    }
    Ok(cert)
}

/// Go: `alreadyInChain`: a certificate with the same subject and key is in the chain, and both
/// lack a subject alternative name or carry the same one. Go stops at the first certificate with
/// the same subject and key where only one of the two has the extension.
fn already_in_chain(candidate: &Certificate, chain: &[&Certificate]) -> bool {
    for cert in chain {
        if cert.subject != candidate.subject || cert.spki != candidate.spki {
            continue;
        }
        match (&candidate.san, &cert.san) {
            (None, None) => return true,
            (Some(a), Some(b)) if a == b => return true,
            (Some(_), Some(_)) => {}
            (None, Some(_)) | (Some(_), None) => return false,
        }
    }
    false
}

/// Go: `UnknownAuthorityError` with the first error that rejected a candidate issuer.
fn unknown_authority(hint: Option<(String, &Certificate)>) -> String {
    match hint {
        None => UNKNOWN_AUTHORITY.to_owned(),
        Some((reason, candidate)) => format!(
            "{UNKNOWN_AUTHORITY} (possibly because of {} while trying to verify candidate \
             authority certificate {})",
            quote(&reason),
            quote(&candidate.display_name())
        ),
    }
}

/// Go: `isValid` for a certificate of `kind` above `chain` (empty for the leaf). Go's check that
/// the child's issuer is this certificate's subject always passes here: candidates are found by
/// that name.
fn is_valid(
    cert: &Certificate,
    kind: Kind,
    chain: &[&Certificate],
    now: &Time,
) -> Result<(), String> {
    if cert.unhandled_critical {
        return Err(UNHANDLED_CRITICAL.to_owned());
    }
    if now.before(&cert.not_before) {
        return Err(format!(
            "{EXPIRED}current time {} is before {}",
            now.format(time::RFC3339),
            cert.not_before.format(time::RFC3339)
        ));
    }
    if now.after(&cert.not_after) {
        return Err(format!(
            "{EXPIRED}current time {} is after {}",
            now.format(time::RFC3339),
            cert.not_after.format(time::RFC3339)
        ));
    }
    if kind == Kind::Intermediate && !cert.basic_constraints.is_some_and(|bc| bc.is_ca) {
        return Err(NOT_AUTHORIZED_TO_SIGN.to_owned());
    }
    // Go: `len(currentChain) - 1` intermediates lie below (-1 for the leaf).
    let below = chain
        .len()
        .checked_sub(1)
        .and_then(|n| u64::try_from(n).ok());
    if let Some(max) = cert.basic_constraints.and_then(|bc| bc.max_path_len)
        && below.is_some_and(|intermediates| intermediates > max)
    {
        return Err(TOO_MANY_INTERMEDIATES.to_owned());
    }
    Ok(())
}

/// Go: `CheckSignatureFrom`: `parent` may sign certificates, and its key verifies `child`'s
/// signature. (Go's check of the parent's key algorithm is moot: `buildChains` skips parents
/// without a key.)
fn check_signature_from(
    child: &Certificate,
    parent: &Certificate,
    algs: &WebPkiSupportedAlgorithms,
) -> Result<(), String> {
    let ca = parent
        .basic_constraints
        .map_or(parent.version != 3, |bc| bc.is_ca);
    if !ca || (parent.key_usage != 0 && parent.key_usage & KEY_USAGE_CERT_SIGN == 0) {
        return Err(CONSTRAINT_VIOLATION.to_owned());
    }
    check_signature(child, parent, algs)
}

/// Go's hashes, as `signatureAlgorithmDetails` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hash {
    /// Go: `crypto.Hash(0)`: Ed25519 hashes the message itself.
    None,
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

/// Go: `PublicKeyAlgorithm`, with `String`'s names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAlgorithm {
    Unknown,
    Rsa,
    Dsa,
    Ecdsa,
    Ed25519,
}

impl KeyAlgorithm {
    const fn name(self) -> &'static str {
        match self {
            Self::Unknown => "0",
            Self::Rsa => "RSA",
            Self::Dsa => "DSA",
            Self::Ecdsa => "ECDSA",
            Self::Ed25519 => "Ed25519",
        }
    }
}

/// Go: `signatureAlgorithmDetails`: the name, the hash and the key algorithm of a signature
/// algorithm (`Unknown` has none of them).
const fn details(alg: SignatureAlgorithm) -> (&'static str, Hash, KeyAlgorithm) {
    use KeyAlgorithm as K;
    use SignatureAlgorithm as S;
    match alg {
        S::Unknown => ("0", Hash::None, K::Unknown),
        S::Md5WithRsa => ("MD5-RSA", Hash::Md5, K::Rsa),
        S::Sha1WithRsa => ("SHA1-RSA", Hash::Sha1, K::Rsa),
        S::Sha256WithRsa => ("SHA256-RSA", Hash::Sha256, K::Rsa),
        S::Sha384WithRsa => ("SHA384-RSA", Hash::Sha384, K::Rsa),
        S::Sha512WithRsa => ("SHA512-RSA", Hash::Sha512, K::Rsa),
        S::Sha256WithRsaPss => ("SHA256-RSAPSS", Hash::Sha256, K::Rsa),
        S::Sha384WithRsaPss => ("SHA384-RSAPSS", Hash::Sha384, K::Rsa),
        S::Sha512WithRsaPss => ("SHA512-RSAPSS", Hash::Sha512, K::Rsa),
        S::DsaWithSha1 => ("DSA-SHA1", Hash::Sha1, K::Dsa),
        S::DsaWithSha256 => ("DSA-SHA256", Hash::Sha256, K::Dsa),
        S::EcdsaWithSha1 => ("ECDSA-SHA1", Hash::Sha1, K::Ecdsa),
        S::EcdsaWithSha256 => ("ECDSA-SHA256", Hash::Sha256, K::Ecdsa),
        S::EcdsaWithSha384 => ("ECDSA-SHA384", Hash::Sha384, K::Ecdsa),
        S::EcdsaWithSha512 => ("ECDSA-SHA512", Hash::Sha512, K::Ecdsa),
        S::PureEd25519 => ("Ed25519", Hash::None, K::Ed25519),
    }
}

/// The rustls (aws-lc-rs) algorithm for a signature Go would verify: the key's and the
/// signature's algorithm identifiers. `None` for what rustls does not offer (P-224 keys).
fn rustls_algorithm(alg: SignatureAlgorithm, key: PublicKey) -> Option<[AlgorithmIdentifier; 2]> {
    use SignatureAlgorithm as S;
    let curve = |c| match c {
        Curve::P256 => Some(alg_id::ECDSA_P256),
        Curve::P384 => Some(alg_id::ECDSA_P384),
        Curve::P521 => Some(alg_id::ECDSA_P521),
        Curve::P224 => None,
    };
    let ids = match (alg, key) {
        (S::Sha256WithRsa, PublicKey::Rsa) => [alg_id::RSA_ENCRYPTION, alg_id::RSA_PKCS1_SHA256],
        (S::Sha384WithRsa, PublicKey::Rsa) => [alg_id::RSA_ENCRYPTION, alg_id::RSA_PKCS1_SHA384],
        (S::Sha512WithRsa, PublicKey::Rsa) => [alg_id::RSA_ENCRYPTION, alg_id::RSA_PKCS1_SHA512],
        (S::Sha256WithRsaPss, PublicKey::Rsa) => [alg_id::RSA_ENCRYPTION, alg_id::RSA_PSS_SHA256],
        (S::Sha384WithRsaPss, PublicKey::Rsa) => [alg_id::RSA_ENCRYPTION, alg_id::RSA_PSS_SHA384],
        (S::Sha512WithRsaPss, PublicKey::Rsa) => [alg_id::RSA_ENCRYPTION, alg_id::RSA_PSS_SHA512],
        (S::EcdsaWithSha256, PublicKey::Ecdsa(c)) => [curve(c)?, alg_id::ECDSA_SHA256],
        (S::EcdsaWithSha384, PublicKey::Ecdsa(c)) => [curve(c)?, alg_id::ECDSA_SHA384],
        (S::EcdsaWithSha512, PublicKey::Ecdsa(c)) => [curve(c)?, alg_id::ECDSA_SHA512],
        (S::PureEd25519, PublicKey::Ed25519) => [alg_id::ED25519, alg_id::ED25519],
        _ => return None,
    };
    Some(ids)
}

/// Verifies `child`'s signature with `parent`'s key using the rustls algorithm for it.
fn rustls_verify(
    child: &Certificate,
    parent: &Certificate,
    algs: &WebPkiSupportedAlgorithms,
) -> Option<bool> {
    let [key, signature] = rustls_algorithm(child.signature_algorithm, parent.public_key)?;
    let alg = algs
        .all
        .iter()
        .find(|a| a.public_key_alg_id() == key && a.signature_alg_id() == signature)?;
    Some(
        alg.verify_signature(&parent.public_key_bytes, &child.tbs, &child.signature)
            .is_ok(),
    )
}

/// Go: `checkSignature(child.SignatureAlgorithm, child.RawTBSCertificate, child.Signature,
/// parent.PublicKey, false)`.
fn check_signature(
    child: &Certificate,
    parent: &Certificate,
    algs: &WebPkiSupportedAlgorithms,
) -> Result<(), String> {
    let (name, hash, key_algorithm) = details(child.signature_algorithm);
    match hash {
        Hash::None if key_algorithm != KeyAlgorithm::Ed25519 => {
            return Err(UNSUPPORTED_ALGORITHM.to_owned());
        }
        Hash::Md5 | Hash::Sha1 => {
            return Err(format!(
                "x509: cannot verify signature: insecure algorithm {name}"
            ));
        }
        Hash::None | Hash::Sha256 | Hash::Sha384 | Hash::Sha512 => {}
    }
    let (want, key_type, failure) = match parent.public_key {
        PublicKey::Rsa => (
            KeyAlgorithm::Rsa,
            "*rsa.PublicKey",
            "crypto/rsa: verification error",
        ),
        PublicKey::Ecdsa(_) => (
            KeyAlgorithm::Ecdsa,
            "*ecdsa.PublicKey",
            "x509: ECDSA verification failure",
        ),
        PublicKey::Ed25519 => (
            KeyAlgorithm::Ed25519,
            "ed25519.PublicKey",
            "x509: Ed25519 verification failure",
        ),
        PublicKey::Dsa | PublicKey::Unknown => return Err(UNSUPPORTED_ALGORITHM.to_owned()),
    };
    if key_algorithm != want {
        return Err(format!(
            "x509: signature algorithm specifies an {} public key, but have public key of type \
             {key_type}",
            key_algorithm.name()
        ));
    }
    if let Some(rsa) = parent.rsa {
        rsa_key_check(rsa)?;
    }
    match rustls_verify(child, parent, algs) {
        Some(true) => Ok(()),
        Some(false) => Err(failure.to_owned()),
        None => Err(UNSUPPORTED_ALGORITHM.to_owned()),
    }
}

/// Go 1.26 `crypto/rsa`'s checks of a key before it verifies (`checkPublicKeySize`, then
/// `checkPublicKey`), and the sizes aws-lc-rs verifies with (2048 to 8192 bits), which Go does
/// not limit.
fn rsa_key_check(key: cert::RsaKey) -> Result<(), String> {
    if key.bits < 1024 {
        return Err(format!(
            "crypto/rsa: {}-bit keys are insecure (see \
             https://go.dev/pkg/crypto/rsa#hdr-Minimum_key_size)",
            key.bits
        ));
    }
    if !key.odd {
        return Err("crypto/rsa: public modulus is even".to_owned());
    }
    if key.exponent < 2 {
        return Err("crypto/rsa: public exponent too small or negative".to_owned());
    }
    if key.exponent % 2 == 0 {
        return Err("crypto/rsa: public exponent is even".to_owned());
    }
    if key.exponent > i64::from(i32::MAX) {
        return Err("crypto/rsa: public exponent too large".to_owned());
    }
    if !(2048..=MAX_RSA_BITS).contains(&key.bits) {
        return Err(UNSUPPORTED_ALGORITHM.to_owned());
    }
    Ok(())
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
    if cert.san.is_none() && match_hostnames(&cert.common_name, host) {
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
    /// The name as the transport has it; rustls's [`ServerName`] drops a trailing dot and the
    /// brackets of an IPv6 address.
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
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let host = self
            .host
            .clone()
            .unwrap_or_else(|| server_name.to_str().into_owned());
        // Go: `Config.time()`, the clock to the nanosecond; rustls's time has whole seconds.
        self.roots
            .verify(end_entity, intermediates, &host, &Time::now())
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
