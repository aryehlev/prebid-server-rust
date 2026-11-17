//! Differential replay of the Go certificate verification goldens.
//!
//! Two Go programs write them, run from the repository root with Go 1.26.2:
//!
//! - `go run ./rust/crates/gohttp-client/testdata/go/x509` writes `testdata/golden/x509.json`:
//!   chains `x509.CreateCertificate` makes (names, validity, usages, pools, pinned leaves).
//! - `go run ./rust/crates/gohttp-client/testdata/go/x509adv` writes
//!   `testdata/golden/x509_adversarial.json`: certificates encoded by hand to reach what
//!   `CreateCertificate` will not make (name constraint values, policies, keys and signature
//!   algorithms Go rejects, encodings at the edges of Go's parser, cross-signed and looping paths).
//!
//! Each case is a server chain, a root pool and a server name, with the error Go's TLS client
//! reports for it (empty when the chain verifies) at the golden's fixed time plus the case's
//! `nanos`.

use std::{collections::BTreeMap, fs, path::Path};

use pbs_gocompat::time::Time;
use rustls::{
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, pem::PemObject as _},
};
use serde_json::Value;

use super::{Roots, hostname_error, match_hostnames, valid_hostname, verify_hostname};
use crate::testutil::ensure_eq;

/// The number of cases in `x509.json`: a truncated file fails the test.
const CASES: usize = 61;

/// The number of cases in `x509_adversarial.json`.
const ADVERSARIAL_CASES: usize = 191;

/// The cases where the port knowingly reports something else than Go (the crate docs list the
/// differences), with what it reports.
const ADVERSARIAL_DIVERGENCES: [(&str, &str); 1] = [
    // aws-lc-rs verifies RSA signatures with keys of 2048 to 8192 bits; Go from 1024 bits.
    (
        "sig_rsa1024",
        "tls: failed to verify certificate: x509: certificate signed by unknown authority \
         (possibly because of \"x509: cannot verify signature: algorithm unimplemented\" while \
         trying to verify candidate authority certificate \"root_rsa1024\")",
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

/// Replays `testdata/golden/<file>`, which must hold `count` cases.
fn replay(file: &str, count: usize, divergences: &[(&str, &str)]) -> Result<(), String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/golden")
        .join(file);
    let content = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let golden: Value = serde_json::from_slice(&content).map_err(|e| e.to_string())?;
    let now = golden
        .get("now")
        .and_then(Value::as_i64)
        .ok_or("missing now")?;
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
    ensure_eq(&cases.len(), &count, "case count")?;
    let algs = aws_lc_rs::default_provider().signature_verification_algorithms;
    let mut failures = Vec::new();
    for case in cases {
        let name = field(case, "name")?;
        let go = field(case, "error")?;
        let pool = ders(case, "roots", &certs)?;
        let chain = ders(case, "chain", &certs)?;
        let (leaf, intermediates) = chain.split_first().ok_or("empty chain")?;
        let nanos = case.get("nanos").and_then(Value::as_i64).unwrap_or(0);
        let at = Time::from_unix(now, nanos);
        let got = Roots::new(&pool, None, algs)
            .verify(leaf, intermediates, field(case, "host")?, &at)
            .err()
            .map_or_else(String::new, |e| e.to_string());
        let divergence = divergences.iter().find(|(n, _)| *n == name);
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

#[test]
fn go_x509_goldens() -> Result<(), String> {
    replay("x509.json", CASES, &[])
}

#[test]
fn go_x509_adversarial_goldens() -> Result<(), String> {
    replay(
        "x509_adversarial.json",
        ADVERSARIAL_CASES,
        &ADVERSARIAL_DIVERGENCES,
    )
}

/// Go: `SystemRootsError`, which a verification against a system pool that did not load reports
/// after the certificates parse.
#[test]
fn system_roots_error() -> Result<(), String> {
    let algs = aws_lc_rs::default_provider().signature_verification_algorithms;
    let roots = Roots::new(
        &[],
        Some("open /etc/ssl/certs/ca-certificates.crt: permission denied".to_owned()),
        algs,
    );
    let now = Time::from_unix(0, 0);
    ensure_eq(
        &roots
            .verify(&CertificateDer::from(Vec::new()), &[], "localhost", &now)
            .map_err(|e| e.to_string()),
        &Err(
            "tls: failed to parse certificate from server: x509: malformed certificate".to_owned(),
        ),
        "parsing comes first",
    )?;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/golden/x509.json");
    let content = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let golden: Value = serde_json::from_slice(&content).map_err(|e| e.to_string())?;
    let pem = golden
        .pointer("/certs/ok")
        .and_then(Value::as_str)
        .ok_or("missing ok")?;
    let leaf = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|e| e.to_string())?;
    ensure_eq(
        &roots
            .verify(&leaf, &[], "localhost", &now)
            .map_err(|e| e.to_string()),
        &Err(
            "tls: failed to verify certificate: x509: failed to load system roots and no roots \
             provided; open /etc/ssl/certs/ca-certificates.crt: permission denied"
                .to_owned(),
        ),
        "system roots error",
    )
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
    leaf.san = Some(Vec::new());
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
        raw: Vec::new(),
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
        san: None,
        dns_names: Vec::new(),
        email_addresses: Vec::new(),
        ip_addresses: Vec::new(),
        uris: Vec::new(),
        server_auth: true,
        unhandled_critical: false,
        tbs: Vec::new(),
        signature_algorithm: super::cert::SignatureAlgorithm::Unknown,
        signature: Vec::new(),
        spki: Vec::new(),
        public_key: super::cert::PublicKey::Unknown,
        public_key_bytes: Vec::new(),
        rsa: None,
        subject_key_id: Vec::new(),
        authority_key_id: Vec::new(),
        name_constraints: None,
        policies: Vec::new(),
        policy_mappings: Vec::new(),
        require_explicit_policy: None,
        inhibit_policy_mapping: None,
        inhibit_any_policy: None,
    }
}
