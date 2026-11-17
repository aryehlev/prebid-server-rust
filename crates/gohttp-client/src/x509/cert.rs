//! Go `x509.ParseCertificate` (`crypto/x509/parser.go`): the fields certificate verification
//! reads, and Go's error texts for a certificate it rejects.
//!
//! The structure is checked as Go checks it, with Go's DER rules (`cryptobyte`: low tag numbers,
//! minimal definite lengths, minimal integers), and so are the extensions verification reads
//! (key usage, basic constraints, subject alternative names, extended key usage) and the ones Go
//! rejects when marked critical (key identifiers, authority information access). Not ported:
//! the public key (Go parses RSA, ECDSA, Ed25519 and X25519 keys here; rustls-webpki checks keys
//! when it verifies signatures), the URI syntax check of URI names, and the value checks of name
//! constraints, which rustls-webpki enforces.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::from_utf8,
};

use pbs_gocompat::{
    strconv::quote,
    time::{self, DateFields, Time},
};

const BOOLEAN: u8 = 0x01;
const INTEGER: u8 = 0x02;
const BIT_STRING: u8 = 0x03;
const OCTET_STRING: u8 = 0x04;
const OBJECT_IDENTIFIER: u8 = 0x06;
const UTF8_STRING: u8 = 0x0C;
const NUMERIC_STRING: u8 = 0x12;
const PRINTABLE_STRING: u8 = 0x13;
const T61_STRING: u8 = 0x14;
const IA5_STRING: u8 = 0x16;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
const BMP_STRING: u8 = 0x1E;
const SEQUENCE: u8 = 0x30;
const SET: u8 = 0x31;
/// `[0]` primitive.
const CONTEXT_0: u8 = 0x80;
/// `[1]` primitive.
const CONTEXT_1: u8 = 0x81;
/// `[2]` primitive.
const CONTEXT_2: u8 = 0x82;
/// `[6]` primitive (a URI general name).
const CONTEXT_6: u8 = 0x86;
/// `[0]` constructed.
const CONTEXT_0_CONSTRUCTED: u8 = 0xA0;
/// `[1]` constructed.
const CONTEXT_1_CONSTRUCTED: u8 = 0xA1;
/// `[3]` constructed.
const CONTEXT_3_CONSTRUCTED: u8 = 0xA3;

/// The DER contents of `id-at-commonName` (2.5.4.3).
const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
/// The DER contents of `id-at-organizationName` (2.5.4.10).
const OID_ORGANIZATION: &[u8] = &[0x55, 0x04, 0x0A];
/// The DER contents of `anyExtendedKeyUsage` (2.5.29.37.0).
const OID_EKU_ANY: &[u8] = &[0x55, 0x1D, 0x25, 0x00];
/// The DER contents of `id-kp-serverAuth` (1.3.6.1.5.5.7.3.1).
const OID_EKU_SERVER_AUTH: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
/// The DER contents of `id-pe-authorityInfoAccess` (1.3.6.1.5.5.7.1.1).
const OID_AUTHORITY_INFO_ACCESS: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x01];
/// The first two DER bytes of the `id-ce` arc (2.5.29).
const ID_CE: [u8; 2] = [0x55, 0x1D];

/// Go: `x509.KeyUsageCertSign`.
pub(super) const KEY_USAGE_CERT_SIGN: u16 = 1 << 5;

const MALFORMED: &str = "x509: malformed certificate";
const MALFORMED_TBS: &str = "x509: malformed tbs certificate";
const MALFORMED_VERSION: &str = "x509: malformed version";
const INVALID_RDN: &str = "x509: invalid RDNSequence";
const INVALID_NAME_CONSTRAINTS: &str = "x509: invalid NameConstraints extension";
const INVALID_AIA: &str = "x509: invalid authority info access";
const INVALID_CRL_DP: &str = "x509: invalid CRL distribution point";
const INVALID_POLICY_CONSTRAINTS: &str = "x509: invalid policy constraints extension";
const INVALID_POLICIES: &str = "x509: invalid certificate policies";
const INVALID_POLICY_MAPPINGS: &str = "x509: invalid policy mappings extension";

/// Go: `cryptobyte.String`, a DER reader.
#[derive(Debug, Clone, Copy)]
struct Der<'a>(&'a [u8]);

/// One DER element.
#[derive(Debug, Clone, Copy)]
struct Element<'a> {
    tag: u8,
    contents: &'a [u8],
    all: &'a [u8],
}

impl<'a> Der<'a> {
    const fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    const fn peek_tag(self) -> Option<u8> {
        self.0.first().copied()
    }

    /// Go: `ReadAnyASN1Element`: a low tag number and a minimal definite length of at most four
    /// bytes.
    fn read_any(&mut self) -> Option<Element<'a>> {
        let input = self.0;
        let (&tag, rest) = input.split_first()?;
        if tag & 0x1F == 0x1F {
            return None;
        }
        let (&first, rest) = rest.split_first()?;
        let (len, rest) = if first & 0x80 == 0 {
            (usize::from(first), rest)
        } else {
            let (bytes, rest) = rest.split_at_checked(usize::from(first & 0x7F))?;
            if bytes.is_empty() || bytes.len() > 4 || bytes.first() == Some(&0) {
                return None;
            }
            let len = bytes
                .iter()
                .fold(0_usize, |n, &b| (n << 8) | usize::from(b));
            if len < 0x80 {
                return None;
            }
            (len, rest)
        };
        let (contents, rest) = rest.split_at_checked(len)?;
        let all = input.get(..input.len() - rest.len())?;
        self.0 = rest;
        Some(Element { tag, contents, all })
    }

    /// The next element when its tag is `tag`.
    fn read_tagged(&mut self, tag: u8) -> Option<Element<'a>> {
        let mut ahead = *self;
        let element = ahead.read_any()?;
        if element.tag != tag {
            return None;
        }
        *self = ahead;
        Some(element)
    }

    /// Go: `ReadASN1`: the contents of the next element, which must have `tag`.
    fn read(&mut self, tag: u8) -> Option<&'a [u8]> {
        self.read_tagged(tag).map(|e| e.contents)
    }

    /// Go: `ReadASN1Element`: all the bytes of the next element, which must have `tag`.
    fn read_element(&mut self, tag: u8) -> Option<&'a [u8]> {
        self.read_tagged(tag).map(|e| e.all)
    }

    /// Go: `ReadOptionalASN1`: `None` when the next element does not have `tag`, `malformed`
    /// when it has but does not read.
    fn read_optional(&mut self, tag: u8, malformed: &str) -> Result<Option<&'a [u8]>, String> {
        if self.peek_tag() == Some(tag) {
            self.read(tag).map(Some).ok_or_else(|| malformed.to_owned())
        } else {
            Ok(None)
        }
    }

    /// Go: `ReadASN1Boolean`.
    fn read_bool(&mut self) -> Option<bool> {
        match self.read(BOOLEAN)? {
            [0] => Some(false),
            [0xFF] => Some(true),
            _ => None,
        }
    }

    /// Go: `ReadASN1Integer` into an `int64` (`tag` lets Go's `ReadASN1Int64WithTag` share it).
    fn read_int(&mut self, tag: u8) -> Option<i64> {
        let bytes = self.read(tag)?;
        if !minimal_integer(bytes) || bytes.len() > 8 {
            return None;
        }
        let init: i64 = if bytes.first().is_some_and(|b| b & 0x80 != 0) {
            -1
        } else {
            0
        };
        Some(bytes.iter().fold(init, |n, &b| (n << 8) | i64::from(b)))
    }

    /// Go: `ReadASN1Integer` into a `uint`, then `parseBasicConstraintsExtension`'s
    /// `mpl > math.MaxInt` check.
    fn read_max_int(&mut self) -> Option<u64> {
        let bytes = self.read(INTEGER)?;
        if !minimal_integer(bytes) || bytes.first().is_some_and(|b| b & 0x80 != 0) {
            return None;
        }
        let digits = match bytes {
            [0, rest @ ..] => rest,
            _ => bytes,
        };
        if digits.len() > 8 {
            return None;
        }
        let n = digits.iter().fold(0_u64, |n, &b| (n << 8) | u64::from(b));
        (n <= i64::MAX.unsigned_abs()).then_some(n)
    }

    /// Go: `ReadASN1ObjectIdentifier`: the DER contents of a well-formed OID.
    fn read_oid(&mut self) -> Option<&'a [u8]> {
        let mut ahead = *self;
        let contents = ahead.read(OBJECT_IDENTIFIER)?;
        let _arcs = oid_arcs(contents)?;
        *self = ahead;
        Some(contents)
    }

    /// Go: `ReadASN1BitString`: the bytes, whose unused trailing bits must be zero.
    fn read_bit_string(&mut self) -> Option<&'a [u8]> {
        let mut ahead = *self;
        let contents = ahead.read(BIT_STRING)?;
        let (&padding, bytes) = contents.split_first()?;
        if padding > 7 || (bytes.is_empty() && padding != 0) {
            return None;
        }
        if bytes
            .last()
            .is_some_and(|last| last & ((1_u8 << padding) - 1) != 0)
        {
            return None;
        }
        *self = ahead;
        Some(bytes)
    }
}

/// Go: `checkASN1Integer`: at least one byte, minimally encoded.
const fn minimal_integer(bytes: &[u8]) -> bool {
    match bytes {
        [] => false,
        [0, next, ..] => *next & 0x80 != 0,
        [0xFF, next, ..] => *next & 0x80 == 0,
        _ => true,
    }
}

/// Go: `readBase128Int`: at most five bytes, below 2^31, without a leading `0x80`.
fn base128(input: &[u8]) -> Option<(u64, &[u8])> {
    let mut n: u64 = 0;
    for (i, &b) in input.iter().enumerate() {
        if i == 5 || n >= 1 << 24 || (i == 0 && b == 0x80) {
            return None;
        }
        n = (n << 7) | u64::from(b & 0x7F);
        if b & 0x80 == 0 {
            return Some((n, input.get(i + 1..)?));
        }
    }
    None
}

/// Go: `ReadASN1ObjectIdentifier`'s arcs.
fn oid_arcs(contents: &[u8]) -> Option<Vec<u64>> {
    let (first, mut rest) = base128(contents)?;
    let mut arcs = if first < 80 {
        vec![first / 40, first % 40]
    } else {
        vec![2, first - 80]
    };
    while !rest.is_empty() {
        let (arc, tail) = base128(rest)?;
        arcs.push(arc);
        rest = tail;
    }
    Some(arcs)
}

/// Go: `asn1.ObjectIdentifier.String`.
fn oid_text(contents: &[u8]) -> String {
    oid_arcs(contents)
        .unwrap_or_default()
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// Go: `isIA5String` (a Go string ranges over runes, so any byte above 0x7f fails).
const fn is_ia5(bytes: &[u8]) -> bool {
    bytes.is_ascii()
}

/// Go: `isPrintable`, with the `*` and `&` Go accepts.
const fn is_printable(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'\''..=b')' | b'+'..=b'/' | b' ' | b':' | b'=' | b'?' | b'*' | b'&'
        )
}

/// Go: `parseASN1String`.
fn asn1_string(tag: u8, value: &[u8]) -> Result<String, String> {
    let ascii = || value.iter().copied().map(char::from).collect::<String>();
    match tag {
        // Go reads T.61 as Latin-1.
        T61_STRING => Ok(ascii()),
        PRINTABLE_STRING if value.iter().all(|&b| is_printable(b)) => Ok(ascii()),
        PRINTABLE_STRING => Err("invalid PrintableString".to_owned()),
        UTF8_STRING => from_utf8(value)
            .map(str::to_owned)
            .map_err(|_utf8| "invalid UTF-8 string".to_owned()),
        BMP_STRING => bmp_string(value).ok_or_else(|| "invalid BMPString".to_owned()),
        IA5_STRING if is_ia5(value) => Ok(ascii()),
        IA5_STRING => Err("invalid IA5String".to_owned()),
        NUMERIC_STRING if value.iter().all(|&b| b.is_ascii_digit() || b == b' ') => Ok(ascii()),
        NUMERIC_STRING => Err("invalid NumericString".to_owned()),
        _ => Err(format!("unsupported string type: {tag}")),
    }
}

/// Go's `BMPString` decoding: UCS-2 without surrogates or noncharacters, a trailing NUL
/// stripped.
fn bmp_string(value: &[u8]) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let value = value.strip_suffix(&[0, 0]).unwrap_or(value);
    let (pairs, _odd) = value.as_chunks::<2>();
    pairs
        .iter()
        .map(|&[high, low]| {
            let point = (u32::from(high) << 8) | u32::from(low);
            if point == 0xFFFE || point == 0xFFFF || (0xFDD0..=0xFDEF).contains(&point) {
                return None;
            }
            // Surrogates (0xd800 to 0xdfff) are not chars.
            char::from_u32(point)
        })
        .collect()
}

/// The names of a `Name` that Go's error texts quote.
#[derive(Debug, Default)]
struct Names {
    common_name: String,
    organization: Option<String>,
}

/// Go: `parseName` and `pkix.Name.FillFromRDNSequence`: the last common name and the first
/// organization.
fn parse_name(raw: &[u8]) -> Result<Names, String> {
    let mut rdns = Der(Der(raw).read(SEQUENCE).ok_or(INVALID_RDN)?);
    let mut names = Names::default();
    while !rdns.is_empty() {
        let mut set = Der(rdns.read(SET).ok_or(INVALID_RDN)?);
        while !set.is_empty() {
            let mut atav = Der(set
                .read(SEQUENCE)
                .ok_or("x509: invalid RDNSequence: invalid attribute")?);
            let typ = atav
                .read_oid()
                .ok_or("x509: invalid RDNSequence: invalid attribute type")?;
            let value = atav
                .read_any()
                .ok_or("x509: invalid RDNSequence: invalid attribute value")?;
            let text = asn1_string(value.tag, value.contents)
                .map_err(|e| format!("x509: invalid RDNSequence: invalid attribute value: {e}"))?;
            if typ == OID_COMMON_NAME {
                names.common_name = text;
            } else if typ == OID_ORGANIZATION && names.organization.is_none() {
                names.organization = Some(text);
            }
        }
    }
    Ok(names)
}

/// Go: `parseTime` with `cryptobyte`'s `ReadASN1UTCTime` and `ReadASN1GeneralizedTime`: the
/// text must be what Go formats back.
fn parse_time(der: &mut Der<'_>) -> Result<Time, String> {
    match der.peek_tag() {
        Some(UTC_TIME) => der
            .read(UTC_TIME)
            .and_then(utc_time)
            .ok_or_else(|| "x509: malformed UTCTime".to_owned()),
        Some(GENERALIZED_TIME) => der
            .read(GENERALIZED_TIME)
            .and_then(|v| canonical_time("20060102150405Z0700", v))
            .ok_or_else(|| "x509: malformed GeneralizedTime".to_owned()),
        _ => Err("x509: unsupported time format".to_owned()),
    }
}

/// A time `layout` parses and formats back to the same text.
fn canonical_time(layout: &str, value: &[u8]) -> Option<Time> {
    let text = from_utf8(value).ok()?;
    let t = time::parse(layout, text).ok()?;
    (t.format(layout) == text).then_some(t)
}

/// Go: `ReadASN1UTCTime`: the minutes-only layout when the one with seconds does not parse,
/// and years from 2050 read as 1950 to 1999.
fn utc_time(value: &[u8]) -> Option<Time> {
    const SECONDS: &str = "060102150405Z0700";
    const MINUTES: &str = "0601021504Z0700";
    let text = from_utf8(value).ok()?;
    let layout = if time::parse(SECONDS, text).is_ok() {
        SECONDS
    } else {
        MINUTES
    };
    let t = canonical_time(layout, value)?;
    if t.year() < 2050 {
        return Some(t);
    }
    let fields = DateFields {
        year: t.year() - 100,
        month: t.month(),
        day: t.day(),
        hour: t.hour(),
        minute: t.minute(),
        second: t.second(),
        nanosecond: t.nanosecond(),
    };
    Some(Time::date(&fields, t.location().clone()))
}

/// Go: `parseAI`.
fn parse_algorithm(der: &[u8]) -> Result<(), String> {
    let mut ai = Der(der);
    let _algorithm = ai.read_oid().ok_or("x509: malformed OID")?;
    if !ai.is_empty() {
        let _parameters = ai.read_any().ok_or("x509: malformed parameters")?;
    }
    Ok(())
}

/// Go: `BasicConstraintsValid` with `IsCA` and `MaxPathLen` (`None`: Go's -1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BasicConstraints {
    pub(super) is_ca: bool,
    pub(super) max_path_len: Option<u64>,
}

/// A parsed certificate: what verification and Go's error texts read.
#[derive(Debug, Clone)]
pub(super) struct Certificate {
    /// Go: `RawIssuer`.
    pub(super) issuer: Vec<u8>,
    /// Go: `RawSubject`.
    pub(super) subject: Vec<u8>,
    /// Go: `Version`: 1 to 3, or `i64::MIN` when the field holds `i64::MAX` (Go's increment
    /// wraps).
    pub(super) version: i64,
    /// The serial number's big-endian magnitude (Go rejects negative ones).
    pub(super) serial: Vec<u8>,
    pub(super) not_before: Time,
    pub(super) not_after: Time,
    /// Go: `Subject.CommonName`.
    pub(super) common_name: String,
    /// Go: `Subject.Organization[0]`.
    pub(super) organization: Option<String>,
    /// Go: `BasicConstraintsValid`, `IsCA` and `MaxPathLen`.
    pub(super) basic_constraints: Option<BasicConstraints>,
    /// Go: `KeyUsage`.
    pub(super) key_usage: u16,
    /// Whether the certificate has a subject alternative name extension.
    pub(super) has_san: bool,
    /// Go: `DNSNames`.
    pub(super) dns_names: Vec<String>,
    /// Go: `IPAddresses`.
    pub(super) ip_addresses: Vec<IpAddr>,
    /// Whether the extended key usages allow server authentication: none listed, `any`, or
    /// `serverAuth` (Go: `checkChainForKeyUsage` with `ExtKeyUsageServerAuth`).
    pub(super) server_auth: bool,
    /// Go: `len(UnhandledCriticalExtensions) > 0`.
    pub(super) unhandled_critical: bool,
}

impl Certificate {
    /// Go: `UnknownAuthorityError`'s name for a candidate issuer: the common name, else the
    /// first organization, else the serial number.
    pub(super) fn display_name(&self) -> String {
        if !self.common_name.is_empty() {
            return self.common_name.clone();
        }
        self.organization
            .clone()
            .unwrap_or_else(|| format!("serial:{}", decimal(&self.serial)))
    }
}

/// The decimal text of a big-endian unsigned integer (Go: `big.Int.String`).
fn decimal(be: &[u8]) -> String {
    // Little-endian decimal digits.
    let mut digits: Vec<u8> = vec![0];
    for &byte in be {
        let mut carry = u32::from(byte);
        for d in &mut digits {
            let v = u32::from(*d) * 256 + carry;
            *d = u8::try_from(v % 10).unwrap_or_default();
            carry = v / 10;
        }
        while carry > 0 {
            digits.push(u8::try_from(carry % 10).unwrap_or_default());
            carry /= 10;
        }
    }
    while digits.len() > 1 && digits.last() == Some(&0) {
        let _zero = digits.pop();
    }
    digits.iter().rev().map(|&d| char::from(b'0' + d)).collect()
}

/// Go: `x509.ParseCertificate`.
pub(super) fn parse(der: &[u8]) -> Result<Certificate, String> {
    let raw = Der(der).read_element(SEQUENCE).ok_or(MALFORMED)?;
    let mut outer = Der(Der(raw).read(SEQUENCE).ok_or(MALFORMED)?);
    let tbs_raw = outer.read_element(SEQUENCE).ok_or(MALFORMED_TBS)?;
    let mut tbs = Der(Der(tbs_raw).read(SEQUENCE).ok_or(MALFORMED_TBS)?);
    let version = parse_version(&mut tbs)?;
    let serial = tbs
        .read(INTEGER)
        .filter(|b| minimal_integer(b))
        .ok_or("x509: malformed serial number")?;
    if serial.first().is_some_and(|b| b & 0x80 != 0) {
        return Err("x509: negative serial number".to_owned());
    }
    let inner_algorithm = tbs
        .read(SEQUENCE)
        .ok_or("x509: malformed signature algorithm identifier")?;
    let outer_algorithm = outer
        .read(SEQUENCE)
        .ok_or("x509: malformed algorithm identifier")?;
    if inner_algorithm != outer_algorithm {
        return Err("x509: inner and outer signature algorithm identifiers don't match".to_owned());
    }
    parse_algorithm(inner_algorithm)?;
    let issuer = tbs.read_element(SEQUENCE).ok_or("x509: malformed issuer")?;
    let _issuer_names = parse_name(issuer)?;
    let mut validity = Der(tbs.read(SEQUENCE).ok_or("x509: malformed validity")?);
    let not_before = parse_time(&mut validity)?;
    let not_after = parse_time(&mut validity)?;
    // Go's text for a malformed subject is "malformed issuer" too.
    let subject = tbs.read_element(SEQUENCE).ok_or("x509: malformed issuer")?;
    let names = parse_name(subject)?;
    parse_spki(&mut tbs)?;
    let mut cert = Certificate {
        issuer: issuer.to_vec(),
        subject: subject.to_vec(),
        version,
        serial: serial.strip_prefix(&[0]).unwrap_or(serial).to_vec(),
        not_before,
        not_after,
        common_name: names.common_name,
        organization: names.organization,
        basic_constraints: None,
        key_usage: 0,
        has_san: false,
        dns_names: Vec::new(),
        ip_addresses: Vec::new(),
        server_auth: true,
        unhandled_critical: false,
    };
    if version > 1 {
        let _issuer_unique_id = tbs.read_optional(CONTEXT_1, "x509: malformed issuerUniqueID")?;
        let _subject_unique_id = tbs.read_optional(CONTEXT_2, "x509: malformed subjectUniqueID")?;
        if version == 3 {
            parse_extensions(&mut tbs, &mut cert)?;
        }
    }
    let _signature = outer.read_bit_string().ok_or("x509: malformed signature")?;
    if raw.len() != der.len() {
        return Err("x509: trailing data".to_owned());
    }
    Ok(cert)
}

/// Go: the version field (`[0] EXPLICIT INTEGER DEFAULT v1`), plus one.
fn parse_version(tbs: &mut Der<'_>) -> Result<i64, String> {
    let version = match tbs.read_optional(CONTEXT_0_CONSTRUCTED, MALFORMED_VERSION)? {
        None => 0,
        Some(explicit) => {
            let mut inner = Der(explicit);
            let v = inner.read_int(INTEGER).ok_or(MALFORMED_VERSION)?;
            if !inner.is_empty() {
                return Err(MALFORMED_VERSION.to_owned());
            }
            v
        }
    };
    if version < 0 {
        return Err(MALFORMED_VERSION.to_owned());
    }
    // Go: `cert.Version++` on an `int`, which wraps, and then only `> 3` is rejected.
    let version = version.wrapping_add(1);
    if version > 3 {
        return Err("x509: invalid version".to_owned());
    }
    Ok(version)
}

/// Go: the `SubjectPublicKeyInfo` structure (the key itself is not parsed).
fn parse_spki(tbs: &mut Der<'_>) -> Result<(), String> {
    let raw = tbs.read_element(SEQUENCE).ok_or("x509: malformed spki")?;
    let mut spki = Der(Der(raw).read(SEQUENCE).ok_or("x509: malformed spki")?);
    let algorithm = spki
        .read(SEQUENCE)
        .ok_or("x509: malformed public key algorithm identifier")?;
    parse_algorithm(algorithm)?;
    let _public_key = spki
        .read_bit_string()
        .ok_or("x509: malformed subjectPublicKey")?;
    Ok(())
}

/// One extension: its OID contents, criticality and value.
#[derive(Debug, Clone, Copy)]
struct Extension<'a> {
    id: &'a [u8],
    critical: bool,
    value: &'a [u8],
}

/// Go: the extensions loop of `parseCertificate`, then `processExtensions`.
fn parse_extensions(tbs: &mut Der<'_>, cert: &mut Certificate) -> Result<(), String> {
    let Some(list) = tbs.read_optional(CONTEXT_3_CONSTRUCTED, "x509: malformed extensions")? else {
        return Ok(());
    };
    let mut list = Der(Der(list)
        .read(SEQUENCE)
        .ok_or("x509: malformed extensions")?);
    let mut extensions: Vec<Extension<'_>> = Vec::new();
    while !list.is_empty() {
        let mut ext = Der(list.read(SEQUENCE).ok_or("x509: malformed extension")?);
        let id = ext
            .read_oid()
            .ok_or("x509: malformed extension OID field")?;
        let critical = if ext.peek_tag() == Some(BOOLEAN) {
            ext.read_bool()
                .ok_or("x509: malformed extension critical field")?
        } else {
            false
        };
        let value = ext
            .read(OCTET_STRING)
            .ok_or("x509: malformed extension value field")?;
        if extensions.iter().any(|e| e.id == id) {
            return Err(format!(
                "x509: certificate contains duplicate extension with OID {}",
                quote(&oid_text(id))
            ));
        }
        extensions.push(Extension {
            id,
            critical,
            value,
        });
    }
    for ext in extensions {
        let unhandled = process_extension(ext, cert)?;
        if ext.critical && unhandled {
            cert.unhandled_critical = true;
        }
    }
    Ok(())
}

/// Go: one step of `processExtensions`; reports whether Go leaves the extension unhandled.
fn process_extension(ext: Extension<'_>, cert: &mut Certificate) -> Result<bool, String> {
    if ext.id == OID_AUTHORITY_INFO_ACCESS {
        if ext.critical {
            return Err("x509: authority info access incorrectly marked critical".to_owned());
        }
        check_authority_info_access(ext.value)?;
        return Ok(false);
    }
    let &[a, b, number] = ext.id else {
        return Ok(true);
    };
    if [a, b] != ID_CE {
        return Ok(true);
    }
    match number {
        15 => cert.key_usage = parse_key_usage(ext.value)?,
        19 => cert.basic_constraints = Some(parse_basic_constraints(ext.value)?),
        17 => {
            cert.has_san = true;
            return parse_san(ext.value, cert).map(|names| !names);
        }
        30 => return name_constraints_unhandled(ext.value),
        31 => check_crl_distribution_points(ext.value)?,
        35 => check_authority_key_id(ext)?,
        36 => check_policy_constraints(ext.value)?,
        37 => cert.server_auth = parse_ext_key_usage(ext.value)?,
        14 => check_subject_key_id(ext)?,
        32 => check_policies(ext.value)?,
        33 => check_policy_mappings(ext.value)?,
        54 => {
            let _inhibit_any_policy = Der(ext.value)
                .read_int(INTEGER)
                .ok_or("x509: invalid inhibit any policy extension")?;
        }
        _ => return Ok(true),
    }
    Ok(false)
}

/// Go: `parseKeyUsageExtension`: the first nine bits.
fn parse_key_usage(value: &[u8]) -> Result<u16, String> {
    let bits = Der(value)
        .read_bit_string()
        .ok_or("x509: invalid key usage")?;
    let mut usage = 0_u16;
    for i in 0..9_u16 {
        let byte = bits.get(usize::from(i / 8)).copied().unwrap_or_default();
        if byte & (0x80 >> (i % 8)) != 0 {
            usage |= 1 << i;
        }
    }
    Ok(usage)
}

/// Go: `parseBasicConstraintsExtension`.
fn parse_basic_constraints(value: &[u8]) -> Result<BasicConstraints, String> {
    const INVALID: &str = "x509: invalid basic constraints";
    let mut seq = Der(Der(value).read(SEQUENCE).ok_or(INVALID)?);
    let is_ca = if seq.peek_tag() == Some(BOOLEAN) {
        seq.read_bool().ok_or(INVALID)?
    } else {
        false
    };
    let max_path_len = if seq.peek_tag() == Some(INTEGER) {
        Some(seq.read_max_int().ok_or(INVALID)?)
    } else {
        None
    };
    Ok(BasicConstraints {
        is_ca,
        max_path_len,
    })
}

/// Go: `parseSANExtension`; reports whether any DNS, email, IP or URI name was found.
fn parse_san(value: &[u8], cert: &mut Certificate) -> Result<bool, String> {
    let mut list = Der(Der(value)
        .read(SEQUENCE)
        .ok_or("x509: invalid subject alternative names")?);
    let mut found = false;
    while !list.is_empty() {
        let name = list
            .read_any()
            .ok_or("x509: invalid subject alternative name")?;
        match name.tag ^ 0x80 {
            1 | 6 if is_ia5(name.contents) => found = true,
            1 => return Err("x509: SAN rfc822Name is malformed".to_owned()),
            2 if is_ia5(name.contents) => {
                cert.dns_names
                    .push(name.contents.iter().copied().map(char::from).collect());
                found = true;
            }
            2 => return Err("x509: SAN dNSName is malformed".to_owned()),
            6 => return Err("x509: SAN uniformResourceIdentifier is malformed".to_owned()),
            7 => {
                let ip = if let Ok(v4) = <[u8; 4]>::try_from(name.contents) {
                    IpAddr::V4(Ipv4Addr::from(v4))
                } else if let Ok(v6) = <[u8; 16]>::try_from(name.contents) {
                    IpAddr::V6(Ipv6Addr::from(v6))
                } else {
                    return Err(format!(
                        "x509: cannot parse IP address of length {}",
                        name.contents.len()
                    ));
                };
                cert.ip_addresses.push(ip);
                found = true;
            }
            _ => {}
        }
    }
    Ok(found)
}

/// Go: `parseNameConstraintsExtension`'s structure; reports a name type Go does not handle.
fn name_constraints_unhandled(value: &[u8]) -> Result<bool, String> {
    let mut outer = Der(value);
    let mut top = Der(outer.read(SEQUENCE).ok_or(INVALID_NAME_CONSTRAINTS)?);
    let permitted = top.read_optional(CONTEXT_0_CONSTRUCTED, INVALID_NAME_CONSTRAINTS)?;
    let excluded = top.read_optional(CONTEXT_1_CONSTRUCTED, INVALID_NAME_CONSTRAINTS)?;
    if !outer.is_empty() || !top.is_empty() {
        return Err(INVALID_NAME_CONSTRAINTS.to_owned());
    }
    if permitted.unwrap_or_default().is_empty() && excluded.unwrap_or_default().is_empty() {
        return Err("x509: empty name constraints extension".to_owned());
    }
    let mut unhandled = false;
    for subtrees in [permitted, excluded].into_iter().flatten() {
        let mut list = Der(subtrees);
        while !list.is_empty() {
            let base = Der(list.read(SEQUENCE).ok_or(INVALID_NAME_CONSTRAINTS)?)
                .read_any()
                .ok_or(INVALID_NAME_CONSTRAINTS)?;
            // Go handles DNS ([2]), email ([1]), IP ([7]) and URI ([6]) constraints.
            if !matches!(base.tag, 0x81 | 0x82 | 0x86 | 0x87) {
                unhandled = true;
            }
        }
    }
    Ok(unhandled)
}

/// Go: the CRL distribution points structure of `processExtensions`.
fn check_crl_distribution_points(value: &[u8]) -> Result<(), String> {
    let mut points = Der(Der(value)
        .read(SEQUENCE)
        .ok_or("x509: invalid CRL distribution points")?);
    while !points.is_empty() {
        let mut point = Der(points.read(SEQUENCE).ok_or(INVALID_CRL_DP)?);
        let Some(name) = point.read_optional(CONTEXT_0_CONSTRUCTED, INVALID_CRL_DP)? else {
            continue;
        };
        let mut full = Der(Der(name)
            .read(CONTEXT_0_CONSTRUCTED)
            .ok_or(INVALID_CRL_DP)?);
        while full.peek_tag() == Some(CONTEXT_6) {
            let _uri = full.read(CONTEXT_6).ok_or(INVALID_CRL_DP)?;
        }
    }
    Ok(())
}

/// Go: `parseAuthorityKeyIdentifier`.
fn check_authority_key_id(ext: Extension<'_>) -> Result<(), String> {
    const INVALID: &str = "x509: invalid authority key identifier";
    if ext.critical {
        return Err("x509: authority key identifier incorrectly marked critical".to_owned());
    }
    let mut akid = Der(Der(ext.value).read(SEQUENCE).ok_or(INVALID)?);
    if akid.peek_tag() == Some(CONTEXT_0) {
        let _key_id = akid.read(CONTEXT_0).ok_or(INVALID)?;
    }
    Ok(())
}

/// Go: the subject key identifier case of `processExtensions`.
fn check_subject_key_id(ext: Extension<'_>) -> Result<(), String> {
    if ext.critical {
        return Err("x509: subject key identifier incorrectly marked critical".to_owned());
    }
    let _key_id = Der(ext.value)
        .read(OCTET_STRING)
        .ok_or("x509: invalid subject key identifier")?;
    Ok(())
}

/// Go: the policy constraints case of `processExtensions`.
fn check_policy_constraints(value: &[u8]) -> Result<(), String> {
    let mut seq = Der(Der(value)
        .read(SEQUENCE)
        .ok_or(INVALID_POLICY_CONSTRAINTS)?);
    for tag in [CONTEXT_0, CONTEXT_1] {
        if seq.peek_tag() == Some(tag) {
            let _skip_certs = seq.read_int(tag).ok_or(INVALID_POLICY_CONSTRAINTS)?;
        }
    }
    Ok(())
}

/// Go: `parseExtKeyUsageExtension`; reports whether server authentication is allowed.
fn parse_ext_key_usage(value: &[u8]) -> Result<bool, String> {
    const INVALID: &str = "x509: invalid extended key usages";
    let mut oids = Der(Der(value).read(SEQUENCE).ok_or(INVALID)?);
    let mut listed = false;
    let mut server_auth = false;
    while !oids.is_empty() {
        let oid = oids.read_oid().ok_or(INVALID)?;
        listed = true;
        server_auth |= oid == OID_EKU_ANY || oid == OID_EKU_SERVER_AUTH;
    }
    Ok(server_auth || !listed)
}

/// Go: `parseCertificatePoliciesExtension`, which reads each policy with `newOIDFromDER` (arcs
/// of any size), not `ReadASN1ObjectIdentifier`.
fn check_policies(value: &[u8]) -> Result<(), String> {
    let mut list = Der(Der(value).read(SEQUENCE).ok_or(INVALID_POLICIES)?);
    let mut seen: Vec<&[u8]> = Vec::new();
    while !list.is_empty() {
        let oid = Der(list.read(SEQUENCE).ok_or(INVALID_POLICIES)?)
            .read(OBJECT_IDENTIFIER)
            .ok_or(INVALID_POLICIES)?;
        if seen.contains(&oid) || !oid_der_valid(oid) {
            return Err(INVALID_POLICIES.to_owned());
        }
        seen.push(oid);
    }
    Ok(())
}

/// Go: `newOIDFromDER`: base-128 subidentifiers of any length, each without a leading `0x80`,
/// the last one complete.
fn oid_der_valid(der: &[u8]) -> bool {
    if der.last().is_none_or(|last| last & 0x80 != 0) {
        return false;
    }
    let mut starts_subidentifier = true;
    for &b in der {
        if starts_subidentifier && b == 0x80 {
            return false;
        }
        starts_subidentifier = b & 0x80 == 0;
    }
    true
}

/// Go: the policy mappings case of `processExtensions`.
fn check_policy_mappings(value: &[u8]) -> Result<(), String> {
    let mut list = Der(Der(value).read(SEQUENCE).ok_or(INVALID_POLICY_MAPPINGS)?);
    while !list.is_empty() {
        let mut mapping = Der(list.read(SEQUENCE).ok_or(INVALID_POLICY_MAPPINGS)?);
        let _subject_policy = mapping
            .read(OBJECT_IDENTIFIER)
            .and_then(|_issuer| mapping.read(OBJECT_IDENTIFIER))
            .ok_or(INVALID_POLICY_MAPPINGS)?;
    }
    Ok(())
}

/// Go: the authority information access case of `processExtensions`.
fn check_authority_info_access(value: &[u8]) -> Result<(), String> {
    let mut list = Der(Der(value).read(SEQUENCE).ok_or(INVALID_AIA)?);
    while !list.is_empty() {
        let mut description = Der(list.read(SEQUENCE).ok_or(INVALID_AIA)?);
        let _method = description.read_oid().ok_or(INVALID_AIA)?;
        if description.peek_tag() == Some(CONTEXT_6) {
            let _location = description.read(CONTEXT_6).ok_or(INVALID_AIA)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    #[test]
    fn der_lengths() -> Result<(), String> {
        let long: Vec<u8> = [0x04, 0x81, 0x80].into_iter().chain([7; 0x80]).collect();
        ensure_eq(
            &Der(&long).read(OCTET_STRING).map(<[u8]>::len),
            &Some(0x80),
            "long form",
        )?;
        ensure_eq(
            &Der(&[0x04, 0x81, 0x01, 7]).read(OCTET_STRING),
            &None,
            "long form for a short length",
        )?;
        ensure_eq(
            &Der(&[0x04, 0x80, 7, 0, 0]).read(OCTET_STRING),
            &None,
            "indefinite length",
        )?;
        ensure_eq(
            &Der(&[0x1F, 0x01, 0x00]).read_any().map(|e| e.tag),
            &None,
            "high tag number",
        )
    }

    #[test]
    fn integers_and_oids() -> Result<(), String> {
        ensure_eq(
            &Der(&[2, 1, 0xFF]).read_int(INTEGER),
            &Some(-1),
            "minus one",
        )?;
        ensure_eq(
            &Der(&[2, 2, 0, 0x7F]).read_int(INTEGER),
            &None,
            "not minimal",
        )?;
        ensure_eq(
            &Der(&[2, 2, 0, 0x80]).read_max_int(),
            &Some(128),
            "unsigned",
        )?;
        ensure_eq(&Der(&[2, 1, 0x80]).read_max_int(), &None, "negative")?;
        ensure_eq(
            oid_text(OID_EKU_SERVER_AUTH).as_str(),
            "1.3.6.1.5.5.7.3.1",
            "oid text",
        )?;
        ensure_eq(&oid_arcs(&[0x55, 0x80, 0x01]), &None, "non-minimal arc")?;
        // A 2.25 UUID arc needs more than five base-128 bytes: newOIDFromDER takes it.
        let uuid = [0x69, 0x82, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        ensure_eq(&oid_arcs(&uuid), &None, "ReadASN1ObjectIdentifier")?;
        ensure_eq(&oid_der_valid(&uuid), &true, "newOIDFromDER")?;
        ensure_eq(&oid_der_valid(&[0x2A, 0x80, 0x01]), &false, "leading 0x80")?;
        ensure_eq(&oid_der_valid(&[0x2A, 0x81]), &false, "truncated")?;
        ensure_eq(&oid_der_valid(&[]), &false, "empty")
    }

    #[test]
    fn versions() -> Result<(), String> {
        let version = |v: &[u8]| {
            let explicit: Vec<u8> = [0xA0, u8::try_from(v.len() + 2).unwrap_or_default(), 0x02]
                .into_iter()
                .chain([u8::try_from(v.len()).unwrap_or_default()])
                .chain(v.iter().copied())
                .collect();
            parse_version(&mut Der(&explicit))
        };
        ensure_eq(&parse_version(&mut Der(&[])), &Ok(1), "absent")?;
        ensure_eq(&version(&[2]), &Ok(3), "v3")?;
        ensure_eq(
            &version(&[3]),
            &Err("x509: invalid version".to_owned()),
            "v4",
        )?;
        ensure_eq(
            &version(&[0xFF]),
            &Err(MALFORMED_VERSION.to_owned()),
            "negative",
        )?;
        // Go's `cert.Version++` wraps.
        ensure_eq(
            &version(&[0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            &Ok(i64::MIN),
            "int64 maximum",
        )
    }

    #[test]
    fn strings_and_serials() -> Result<(), String> {
        ensure_eq(
            &asn1_string(PRINTABLE_STRING, b"a_b"),
            &Err("invalid PrintableString".to_owned()),
            "printable",
        )?;
        ensure_eq(
            &asn1_string(T61_STRING, &[0xE9]),
            &Ok("\u{e9}".to_owned()),
            "t61",
        )?;
        ensure_eq(
            &asn1_string(BMP_STRING, &[0, b'h', 0, b'i', 0, 0]),
            &Ok("hi".to_owned()),
            "bmp",
        )?;
        ensure_eq(
            &asn1_string(BMP_STRING, &[0xD8, 0]),
            &Err("invalid BMPString".to_owned()),
            "surrogate",
        )?;
        ensure_eq(
            &asn1_string(0x04, b"x"),
            &Err("unsupported string type: 4".to_owned()),
            "octet string",
        )?;
        ensure_eq(decimal(&[]).as_str(), "0", "zero")?;
        ensure_eq(
            decimal(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]).as_str(),
            "18446744073709551616",
            "2^64",
        )
    }

    #[test]
    fn times() -> Result<(), String> {
        let rfc3339 = |v: &[u8]| utc_time(v).map(|t| t.format(time::RFC3339));
        ensure_eq(
            &rfc3339(b"491231235959Z"),
            &Some("2049-12-31T23:59:59Z".to_owned()),
            "2049",
        )?;
        ensure_eq(
            &rfc3339(b"500101000000Z"),
            &Some("1950-01-01T00:00:00Z".to_owned()),
            "1950",
        )?;
        ensure_eq(
            &rfc3339(b"2601010000Z"),
            &Some("2026-01-01T00:00:00Z".to_owned()),
            "minutes",
        )?;
        ensure_eq(&rfc3339(b"260101000000.5Z"), &None, "fraction")?;
        ensure_eq(
            &canonical_time("20060102150405Z0700", b"20260101000000+0100")
                .map(|t| t.format(time::RFC3339)),
            &Some("2026-01-01T00:00:00+01:00".to_owned()),
            "offset",
        )
    }
}
