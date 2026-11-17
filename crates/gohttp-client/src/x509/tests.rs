//! Differential replay of the Go certificate verification goldens.
//!
//! `testdata/golden/x509.json` is written by `testdata/go/x509` (run
//! `go run ./rust/crates/gohttp-client/testdata/go/x509` from the repository root with Go
//! 1.26.2). Each case is a server chain, a root pool and a server name, with the error Go's TLS
//! client reports for it at the golden's fixed time (empty when the chain verifies).

use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use pbs_gocompat::time::Time;
use rustls::{
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, UnixTime, pem::PemObject as _},
};
use serde_json::Value;

use super::{Roots, hostname_error, match_hostnames, valid_hostname, verify_hostname};
use crate::testutil::ensure_eq;

/// The number of cases in the golden: a truncated file fails the test.
const CASES: usize = 61;

/// The cases where the port knowingly reports something else than Go (the crate docs list the
/// differences), with what it reports.
const DIVERGENCES: [(&str, &str); 6] = [
    // Go accepts a CA certificate as the leaf of a chain to a root; webpki rejects it.
    (
        "leaf_is_ca",
        "tls: failed to verify certificate: x509: certificate signed by unknown authority",
    ),
    // Go names the pooled CA whose key does not match the signature.
    (
        "same_name_other_key",
        "tls: failed to verify certificate: x509: certificate signed by unknown authority",
    ),
    // Go handles a critical certificate policies extension; webpki rejects it outside a trust
    // anchor (the root_policies_critical case verifies).
    (
        "leaf_policies_critical",
        "tls: failed to verify certificate: x509: unhandled critical extension",
    ),
    (
        "intermediate_policies_critical",
        "tls: failed to verify certificate: x509: unhandled critical extension",
    ),
    // Go reads an empty extended key usage list as no restriction; webpki rejects it.
    (
        "leaf_empty_eku",
        "tls: failed to verify certificate: x509: EmptyEkuExtension",
    ),
    // Go names the name and the constraint; webpki only reports the violation.
    (
        "intermediate_name_constraints",
        "tls: failed to verify certificate: x509: NameConstraintViolation",
    ),
];

fn field<'a>(case: &'a Value, key: &str) -> Result<&'a str, String> {
    case.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}

fn ders(
    case: &Value,
    key: &str,
    certs: &BTreeMap<String, CertificateDer<'static>>,
) -> Result<Vec<CertificateDer<'static>>, String> {
    case.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key}"))?
        .iter()
        .map(|name| {
            name.as_str()
                .and_then(|n| certs.get(n))
                .cloned()
                .ok_or_else(|| format!("unknown certificate {name}"))
        })
        .collect()
}

#[test]
fn go_x509_goldens() -> Result<(), String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/golden/x509.json");
    let content = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let golden: Value = serde_json::from_slice(&content).map_err(|e| e.to_string())?;
    let now = golden
        .get("now")
        .and_then(Value::as_u64)
        .ok_or("missing now")?;
    let now = UnixTime::since_unix_epoch(Duration::from_secs(now));
    let mut certs = BTreeMap::new();
    for (name, pem) in golden
        .get("certs")
        .and_then(Value::as_object)
        .ok_or("missing certs")?
    {
        let der = pem
            .as_str()
            .and_then(|p| CertificateDer::from_pem_slice(p.as_bytes()).ok())
            .ok_or_else(|| format!("bad PEM for {name}"))?;
        let _replaced = certs.insert(name.clone(), der);
    }
    let cases = golden
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("missing cases")?;
    ensure_eq(&cases.len(), &CASES, "case count")?;
    let algs = aws_lc_rs::default_provider().signature_verification_algorithms;
    let mut failures = Vec::new();
    for case in cases {
        let name = field(case, "name")?;
        let go = field(case, "error")?;
        let pool = ders(case, "roots", &certs)?;
        let chain = ders(case, "chain", &certs)?;
        let (leaf, intermediates) = chain.split_first().ok_or("empty chain")?;
        let got = Roots::new(&pool, algs)
            .verify(leaf, intermediates, field(case, "host")?, now)
            .err()
            .map_or_else(String::new, |e| e.to_string());
        let divergence = DIVERGENCES.iter().find(|(n, _)| *n == name);
        if divergence.is_some_and(|(_, text)| *text == go) {
            failures.push(format!("{name}: listed as a divergence but Go agrees"));
        }
        let want = divergence.map_or(go, |(_, text)| *text);
        if got != want {
            failures.push(format!("{name}:\n  got  {got:?}\n  want {want:?}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// Go's hostname rules where the golden certificates do not reach.
#[test]
fn hostname_rules() -> Result<(), String> {
    ensure_eq(&valid_hostname("a_b.example", false), &true, "underscore")?;
    ensure_eq(
        &valid_hostname("-a.example", false),
        &false,
        "leading hyphen",
    )?;
    ensure_eq(
        &valid_hostname("*.example", true),
        &true,
        "wildcard pattern",
    )?;
    ensure_eq(
        &valid_hostname("a.*.example", true),
        &false,
        "inner wildcard",
    )?;
    ensure_eq(&valid_hostname("*", true), &false, "bare wildcard")?;
    ensure_eq(
        &match_hostnames("*.Example.com", "a.example.COM."),
        &true,
        "case and dot",
    )?;
    ensure_eq(
        &match_hostnames("*.example.com", "example.com"),
        &false,
        "no label for *",
    )?;
    ensure_eq(
        &match_hostnames("*.example.com", "a.b.example.com"),
        &false,
        "one label",
    )
}

/// Go matches names it does not consider valid host names exactly, and quotes the host as
/// given.
#[test]
fn hostname_texts() -> Result<(), String> {
    ensure_eq(
        &super::cert::parse(&[]).is_err(),
        &true,
        "empty certificate",
    )?;
    let mut leaf = blank_certificate();
    leaf.dns_names = vec!["exa mple".to_owned(), "*.example.com".to_owned()];
    leaf.has_san = true;
    ensure_eq(&verify_hostname(&leaf, "EXA MPLE"), &Ok(()), "exact match")?;
    ensure_eq(
        &verify_hostname(&leaf, "x.y.example.com"),
        &Err(
            "x509: certificate is valid for exa mple, *.example.com, not x.y.example.com"
                .to_owned(),
        ),
        "dns mismatch",
    )?;
    leaf.ip_addresses = vec!["::ffff:10.0.0.1".parse().map_err(|e| format!("{e}"))?];
    ensure_eq(
        &verify_hostname(&leaf, "[10.0.0.1]"),
        &Ok(()),
        "mapped address in brackets",
    )?;
    ensure_eq(
        hostname_error(&leaf, "10.0.0.2").as_str(),
        "x509: certificate is valid for 10.0.0.1, not 10.0.0.2",
        "mapped address text",
    )
}

/// A certificate with no names, valid for all time.
fn blank_certificate() -> super::cert::Certificate {
    super::cert::Certificate {
        issuer: Vec::new(),
        subject: Vec::new(),
        version: 3,
        serial: vec![1],
        not_before: Time::from_unix(0, 0),
        not_after: Time::from_unix(4_102_444_800, 0),
        common_name: String::new(),
        organization: None,
        basic_constraints: None,
        key_usage: 0,
        has_san: false,
        dns_names: Vec::new(),
        ip_addresses: Vec::new(),
        server_auth: true,
        unhandled_critical: false,
    }
}
