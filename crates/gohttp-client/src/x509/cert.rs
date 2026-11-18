//! Go `x509.ParseCertificate` (`crypto/x509/parser.go`): the fields certificate verification
//! reads, and Go's error texts for a certificate it rejects.
//!
//! The structure is checked as Go checks it, with Go's DER rules (`cryptobyte`: low tag numbers,
//! minimal definite lengths, minimal integers), and so is every extension Go reads: key usage,
//! basic constraints, subject alternative names (URI names parsed with Go's `url.Parse`), name
//! constraints with their values, extended key usage, key identifiers, the policy extensions,
//! and the ones Go rejects when marked critical. The public key is parsed as Go's
//! `parsePublicKey` parses it (RSA, ECDSA on Go's four curves, Ed25519 and DSA; other algorithms
//! leave no key, as in Go), with Go's texts for a key it rejects.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::from_utf8,
};

use pbs_gocompat::{
    strconv::{quote, quote_bytes},
    time::{self, DateFields, Time},
    url::Url,
};

use super::{
    constraints::{domain_name_valid, hex, parse_mailbox},
    ec::{self, Curve},
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
        int64(self.read(tag)?)
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
    fn read_bit_string(&mut self) -> Option<BitString<'a>> {
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
        Some(BitString { bytes, padding })
    }
}

/// Go: `asn1.BitString`: the bytes and the number of unused bits at the end.
#[derive(Debug, Clone, Copy)]
struct BitString<'a> {
    bytes: &'a [u8],
    padding: u8,
}

impl BitString<'_> {
    /// Go: `BitString.RightAlign`: the bits shifted right so that the unused ones lead, as Go
    /// reads signatures and public keys.
    fn right_align(self) -> Vec<u8> {
        if self.padding == 0 {
            return self.bytes.to_vec();
        }
        let shift = self.padding;
        let mut carry = 0_u8;
        self.bytes
            .iter()
            .map(|&b| {
                let aligned = carry | (b >> shift);
                carry = b << (8 - shift);
                aligned
            })
            .collect()
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

/// An INTEGER's contents as an `int64` (Go: `ReadASN1Integer` and `encoding/asn1`'s
/// `parseInt64`).
fn int64(bytes: &[u8]) -> Option<i64> {
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

/// An algorithm identifier: the OID's contents and the parameters' whole element (Go:
/// `pkix.AlgorithmIdentifier` with `Parameters.FullBytes`).
#[derive(Debug, Clone, Copy)]
struct Algorithm<'a> {
    oid: &'a [u8],
    params: Option<&'a [u8]>,
}

/// Go: `parseAI`.
fn parse_algorithm(der: &[u8]) -> Result<Algorithm<'_>, String> {
    let mut ai = Der(der);
    let oid = ai.read_oid().ok_or("x509: malformed OID")?;
    let params = if ai.is_empty() {
        None
    } else {
        Some(ai.read_any().ok_or("x509: malformed parameters")?.all)
    };
    Ok(Algorithm { oid, params })
}

/// Go: `encoding/asn1` reading a `pkix.AlgorithmIdentifier` inside RSA-PSS parameters: an OID,
/// then any element as the parameters, and Go ignores what follows inside the SEQUENCE.
fn algorithm_in<'a>(der: &mut Der<'a>) -> Option<Algorithm<'a>> {
    let mut seq = Der(der.read(SEQUENCE)?);
    let oid = seq.read_oid()?;
    let params = if seq.is_empty() {
        None
    } else {
        Some(seq.read_any()?.all)
    };
    Some(Algorithm { oid, params })
}

/// Go: `encoding/asn1`'s `tagAndLength`.
#[derive(Debug, Clone, Copy)]
struct Asn1Header {
    class: u8,
    constructed: bool,
    number: u64,
    len: usize,
}

impl Asn1Header {
    /// A context-specific tag `[number]`.
    const fn is_context(self, number: u64) -> bool {
        self.class == 2 && self.number == number
    }
}

/// Go: `encoding/asn1`'s `parseTagAndLength`: it reads high tag numbers, and its callers check
/// the length against the input only for the element they read the contents of.
fn asn1_header(input: &[u8]) -> Option<(Asn1Header, &[u8])> {
    let (&first, mut rest) = input.split_first()?;
    let mut number = u64::from(first & 0x1F);
    if number == 0x1F {
        let (high, after) = base128(rest)?;
        if high < 0x1F {
            return None;
        }
        number = high;
        rest = after;
    }
    let (&length, mut rest) = rest.split_first()?;
    let len = if length & 0x80 == 0 {
        usize::from(length)
    } else {
        let (bytes, after) = rest.split_at_checked(usize::from(length & 0x7F))?;
        rest = after;
        let mut len = 0_usize;
        for &b in bytes {
            if len >= 1 << 23 {
                return None;
            }
            len = (len << 8) | usize::from(b);
            if len == 0 {
                return None;
            }
        }
        if len < 0x80 {
            return None;
        }
        len
    };
    let header = Asn1Header {
        class: first >> 6,
        constructed: first & 0x20 != 0,
        number,
        len,
    };
    Some((header, rest))
}

impl Der<'_> {
    /// Go: `encoding/asn1`'s explicit tag `[number]` on a field without a default: a
    /// constructed element with contents, whose length Go ignores, and the field's element is
    /// read from the bytes after its header.
    fn enter_explicit(&mut self, number: u64) -> Option<()> {
        let (wrapper, inner) = asn1_header(self.0)?;
        if inner.is_empty()
            || !wrapper.is_context(number)
            || !wrapper.constructed
            || wrapper.len == 0
        {
            return None;
        }
        self.0 = inner;
        Some(())
    }
}

/// Go: `pssParameters.TrailerField`, `asn1:"optional,explicit,tag:3,default:1"`, read from what
/// follows the salt length: an element that does not match leaves the default, but a header
/// Go cannot read, an explicit tag with nothing after it, an empty `[3]`, or an INTEGER Go
/// cannot read fails the parameters.
fn trailer_field(rest: &[u8]) -> Option<i64> {
    if rest.is_empty() {
        return Some(1);
    }
    let (wrapper, inner) = asn1_header(rest)?;
    if inner.is_empty() {
        return None;
    }
    if !wrapper.is_context(3) || !(wrapper.constructed || wrapper.len == 0) {
        return Some(1);
    }
    if wrapper.len == 0 {
        return None;
    }
    let (int, contents) = asn1_header(inner)?;
    if int.class != 0 || int.number != u64::from(INTEGER) || int.constructed {
        return Some(1);
    }
    int64(contents.get(..int.len)?)
}

const NULL: &[u8] = &[0x05, 0x00];
const OID_RSA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
const OID_RSA_PSS: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0A];
const OID_MGF1: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x08];
const OID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const OID_SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
const OID_SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
const OID_DSA: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x38, 0x04, 0x01];
const OID_ECDSA: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
const OID_ED25519: &[u8] = &[0x2B, 0x65, 0x70];

/// Go: `x509.SignatureAlgorithm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SignatureAlgorithm {
    Unknown,
    Md5WithRsa,
    Sha1WithRsa,
    Sha256WithRsa,
    Sha384WithRsa,
    Sha512WithRsa,
    Sha256WithRsaPss,
    Sha384WithRsaPss,
    Sha512WithRsaPss,
    DsaWithSha1,
    DsaWithSha256,
    EcdsaWithSha1,
    EcdsaWithSha256,
    EcdsaWithSha384,
    EcdsaWithSha512,
    PureEd25519,
}

/// Go: `getSignatureAlgorithmFromAI`.
fn signature_algorithm(ai: Algorithm<'_>) -> SignatureAlgorithm {
    use SignatureAlgorithm as S;
    const RSA_PKCS1: [u8; 8] = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01];
    const ECDSA: [u8; 6] = [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04];
    if ai.oid == OID_ED25519 {
        return if ai.params.is_some() {
            S::Unknown
        } else {
            S::PureEd25519
        };
    }
    if ai.oid == OID_RSA_PSS {
        return pss_algorithm(ai.params);
    }
    match ai.oid {
        [p @ .., 0x04] if p == RSA_PKCS1 => S::Md5WithRsa,
        [p @ .., 0x05] if p == RSA_PKCS1 => S::Sha1WithRsa,
        [0x2B, 0x0E, 0x03, 0x02, 0x1D] => S::Sha1WithRsa,
        [p @ .., 0x0B] if p == RSA_PKCS1 => S::Sha256WithRsa,
        [p @ .., 0x0C] if p == RSA_PKCS1 => S::Sha384WithRsa,
        [p @ .., 0x0D] if p == RSA_PKCS1 => S::Sha512WithRsa,
        [0x2A, 0x86, 0x48, 0xCE, 0x38, 0x04, 0x03] => S::DsaWithSha1,
        [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x02] => S::DsaWithSha256,
        [p @ .., 0x01] if p == ECDSA => S::EcdsaWithSha1,
        [p @ .., 0x03, 0x02] if p == ECDSA => S::EcdsaWithSha256,
        [p @ .., 0x03, 0x03] if p == ECDSA => S::EcdsaWithSha384,
        [p @ .., 0x03, 0x04] if p == ECDSA => S::EcdsaWithSha512,
        _ => S::Unknown,
    }
}

/// Go: the RSA-PSS case of `getSignatureAlgorithmFromAI`, which reads the parameters with
/// `encoding/asn1` (see [`Der::enter_explicit`] and [`trailer_field`]): the MGF1 hash must be the
/// message hash, the salt as long as the hash, and the trailer field the default.
fn pss_algorithm(params: Option<&[u8]>) -> SignatureAlgorithm {
    let absent_or_null = |p: Option<&[u8]>| p.is_none_or(|p| p == NULL);
    let parsed = (|| {
        let mut fields = Der(Der(params?).read(SEQUENCE)?);
        fields.enter_explicit(0)?;
        let hash = algorithm_in(&mut fields)?;
        fields.enter_explicit(1)?;
        let mgf = algorithm_in(&mut fields)?;
        fields.enter_explicit(2)?;
        let salt = fields.read_int(INTEGER)?;
        let trailer = trailer_field(fields.0)?;
        let mgf_hash = algorithm_in(&mut Der(mgf.params?))?;
        let valid = absent_or_null(hash.params)
            && mgf.oid == OID_MGF1
            && mgf_hash.oid == hash.oid
            && absent_or_null(mgf_hash.params)
            && trailer == 1;
        valid.then_some((hash.oid, salt))
    })();
    match parsed {
        Some((OID_SHA256, 32)) => SignatureAlgorithm::Sha256WithRsaPss,
        Some((OID_SHA384, 48)) => SignatureAlgorithm::Sha384WithRsaPss,
        Some((OID_SHA512, 64)) => SignatureAlgorithm::Sha512WithRsaPss,
        Some(_) | None => SignatureAlgorithm::Unknown,
    }
}

/// Go: the type of `Certificate.PublicKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PublicKey {
    /// Go: `nil`, for an algorithm `x509` does not parse (X25519 among them).
    Unknown,
    /// Go: `*rsa.PublicKey`.
    Rsa,
    /// Go: `*dsa.PublicKey`.
    Dsa,
    /// Go: `*ecdsa.PublicKey`.
    Ecdsa(Curve),
    /// Go: `ed25519.PublicKey`.
    Ed25519,
}

/// A big-endian INTEGER's contents, minimally encoded (Go: `ReadASN1Integer` into a `*big.Int`).
fn read_big(der: &mut Der<'_>) -> Option<Vec<u8>> {
    der.read(INTEGER)
        .filter(|b| minimal_integer(b))
        .map(<[u8]>::to_vec)
}

/// Whether a minimal two's complement integer is above zero.
fn positive(int: &[u8]) -> bool {
    int.first().is_some_and(|b| b & 0x80 == 0) && int.iter().any(|&b| b != 0)
}

/// The bit length of a positive minimal integer.
fn bit_len(int: &[u8]) -> usize {
    let digits = int.strip_prefix(&[0]).unwrap_or(int);
    digits.first().map_or(0, |&top| {
        digits.len() * 8 - usize::try_from(top.leading_zeros()).unwrap_or(0)
    })
}

/// What Go's RSA verification checks of a key (`crypto/rsa`'s `checkPublicKeySize` and
/// `checkPublicKey`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RsaKey {
    /// Go: `N.BitLen()`.
    pub(super) bits: usize,
    /// Whether the modulus is odd.
    pub(super) odd: bool,
    /// Go: `E`.
    pub(super) exponent: i64,
}

/// Go: `parsePublicKey`; returns the key's type and, for RSA, what verification checks.
fn parse_public_key(ai: Algorithm<'_>, data: &[u8]) -> Result<(PublicKey, Option<RsaKey>), String> {
    match ai.oid {
        OID_RSA => {
            if ai.params != Some(NULL) {
                return Err("x509: RSA key missing NULL parameters".to_owned());
            }
            let mut der = Der(Der(data)
                .read(SEQUENCE)
                .ok_or("x509: invalid RSA public key")?);
            let n = read_big(&mut der).ok_or("x509: invalid RSA modulus")?;
            let e = der
                .read_int(INTEGER)
                .ok_or("x509: invalid RSA public exponent")?;
            if !positive(&n) {
                return Err("x509: RSA modulus is not a positive number".to_owned());
            }
            if e <= 0 {
                return Err("x509: RSA public exponent is not a positive number".to_owned());
            }
            let rsa = RsaKey {
                bits: bit_len(&n),
                odd: n.last().is_some_and(|b| b & 1 == 1),
                exponent: e,
            };
            Ok((PublicKey::Rsa, Some(rsa)))
        }
        OID_ECDSA => {
            let curve_oid = Der(ai.params.unwrap_or_default())
                .read_oid()
                .ok_or("x509: invalid ECDSA parameters")?;
            let curve = Curve::from_oid(curve_oid).ok_or("x509: unsupported elliptic curve")?;
            ec::check_public_key(curve, data)?;
            Ok((PublicKey::Ecdsa(curve), None))
        }
        OID_ED25519 => {
            if ai.params.is_some() {
                return Err("x509: Ed25519 key encoded with illegal parameters".to_owned());
            }
            if data.len() != 32 {
                return Err("x509: wrong Ed25519 public key size".to_owned());
            }
            Ok((PublicKey::Ed25519, None))
        }
        OID_DSA => {
            let y = read_big(&mut Der(data)).ok_or("x509: invalid DSA public key")?;
            let mut outer = Der(ai.params.unwrap_or_default());
            let mut params = Der(outer.read(SEQUENCE).ok_or("x509: invalid DSA parameters")?);
            let mut pqg = Vec::with_capacity(3);
            for _ in 0..3 {
                pqg.push(read_big(&mut params).ok_or("x509: invalid DSA parameters")?);
            }
            if !(positive(&y) && pqg.iter().all(|v| positive(v))) {
                return Err("x509: zero or negative DSA parameter".to_owned());
            }
            Ok((PublicKey::Dsa, None))
        }
        _ => Ok((PublicKey::Unknown, None)),
    }
}

/// Go: `BasicConstraintsValid` with `IsCA` and `MaxPathLen` (`None`: Go's -1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BasicConstraints {
    pub(super) is_ca: bool,
    pub(super) max_path_len: Option<u64>,
}

/// Go: `PolicyMapping`: the issuer's and the subject's domain policy, as the OIDs' contents.
pub(super) type PolicyMapping = (Vec<u8>, Vec<u8>);

/// A parsed certificate: what verification and Go's error texts read.
#[derive(Debug, Clone)]
pub(super) struct Certificate {
    /// Go: `Raw`, the whole certificate.
    pub(super) raw: Vec<u8>,
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
    /// The subject alternative name extension's value (Go: `hasSANExtension`, and what
    /// `alreadyInChain` compares).
    pub(super) san: Option<Vec<u8>>,
    /// Go: `DNSNames`.
    pub(super) dns_names: Vec<String>,
    /// Go: `EmailAddresses`.
    pub(super) email_addresses: Vec<String>,
    /// Go: `IPAddresses` (four bytes are IPv4, sixteen IPv6).
    pub(super) ip_addresses: Vec<IpAddr>,
    /// Go: `URIs`.
    pub(super) uris: Vec<Url>,
    /// Whether the extended key usages allow server authentication: none listed, `any`, or
    /// `serverAuth` (Go: `checkChainForKeyUsage` with `ExtKeyUsageServerAuth`).
    pub(super) server_auth: bool,
    /// Go: `len(UnhandledCriticalExtensions) > 0`.
    pub(super) unhandled_critical: bool,
    /// Go: `RawTBSCertificate`, what the signature covers.
    pub(super) tbs: Vec<u8>,
    /// Go: `SignatureAlgorithm`.
    pub(super) signature_algorithm: SignatureAlgorithm,
    /// Go: `Signature` (right-aligned).
    pub(super) signature: Vec<u8>,
    /// Go: `RawSubjectPublicKeyInfo`.
    pub(super) spki: Vec<u8>,
    /// The type of Go's `PublicKey`.
    pub(super) public_key: PublicKey,
    /// The subject public key's bits, right-aligned as Go parses them, which is what signature
    /// algorithms take.
    pub(super) public_key_bytes: Vec<u8>,
    /// What Go's RSA verification checks of an RSA key.
    pub(super) rsa: Option<RsaKey>,
    /// Go: `SubjectKeyId`.
    pub(super) subject_key_id: Vec<u8>,
    /// Go: `AuthorityKeyId`.
    pub(super) authority_key_id: Vec<u8>,
    /// Go's name constraint fields, when the extension is present (Go: `hasNameConstraints`).
    pub(super) name_constraints: Option<NameConstraints>,
    /// Go: `Policies`, as DER contents.
    pub(super) policies: Vec<Vec<u8>>,
    /// Go: `PolicyMappings`: issuer and subject domain policies.
    pub(super) policy_mappings: Vec<PolicyMapping>,
    /// Go: `RequireExplicitPolicy`, when the field is present.
    pub(super) require_explicit_policy: Option<i64>,
    /// Go: `InhibitPolicyMapping`, when the field is present.
    pub(super) inhibit_policy_mapping: Option<i64>,
    /// Go: `InhibitAnyPolicy`, when the extension is present.
    pub(super) inhibit_any_policy: Option<i64>,
}

/// An IP range of a name constraint: the address and mask as the certificate holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IpNet {
    pub(super) ip: Vec<u8>,
    pub(super) mask: Vec<u8>,
}

/// The permitted or excluded names of a name constraints extension, by Go's types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Subtrees {
    pub(super) dns: Vec<String>,
    pub(super) ips: Vec<IpNet>,
    pub(super) emails: Vec<String>,
    pub(super) uris: Vec<String>,
}

/// Go: `Permitted...` and `Excluded...`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct NameConstraints {
    pub(super) permitted: Subtrees,
    pub(super) excluded: Subtrees,
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
    let signature_algorithm = signature_algorithm(parse_algorithm(inner_algorithm)?);
    let issuer = tbs.read_element(SEQUENCE).ok_or("x509: malformed issuer")?;
    let _issuer_names = parse_name(issuer)?;
    let mut validity = Der(tbs.read(SEQUENCE).ok_or("x509: malformed validity")?);
    let not_before = parse_time(&mut validity)?;
    let not_after = parse_time(&mut validity)?;
    // Go's text for a malformed subject is "malformed issuer" too.
    let subject = tbs.read_element(SEQUENCE).ok_or("x509: malformed issuer")?;
    let names = parse_name(subject)?;
    let spki = parse_spki(&mut tbs)?;
    let mut cert = Certificate {
        raw: raw.to_vec(),
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
        san: None,
        dns_names: Vec::new(),
        email_addresses: Vec::new(),
        ip_addresses: Vec::new(),
        uris: Vec::new(),
        server_auth: true,
        unhandled_critical: false,
        tbs: tbs_raw.to_vec(),
        signature_algorithm,
        signature: Vec::new(),
        spki: spki.raw.to_vec(),
        public_key: spki.key,
        public_key_bytes: spki.bits,
        rsa: spki.rsa,
        subject_key_id: Vec::new(),
        authority_key_id: Vec::new(),
        name_constraints: None,
        policies: Vec::new(),
        policy_mappings: Vec::new(),
        require_explicit_policy: None,
        inhibit_policy_mapping: None,
        inhibit_any_policy: None,
    };
    if version > 1 {
        let _issuer_unique_id = tbs.read_optional(CONTEXT_1, "x509: malformed issuerUniqueID")?;
        let _subject_unique_id = tbs.read_optional(CONTEXT_2, "x509: malformed subjectUniqueID")?;
        if version == 3 {
            parse_extensions(&mut tbs, &mut cert)?;
        }
    }
    cert.signature = outer
        .read_bit_string()
        .ok_or("x509: malformed signature")?
        .right_align();
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

/// A parsed `SubjectPublicKeyInfo`.
#[derive(Debug, Clone)]
struct Spki<'a> {
    raw: &'a [u8],
    /// The subject public key, right-aligned.
    bits: Vec<u8>,
    key: PublicKey,
    rsa: Option<RsaKey>,
}

/// Go: the `SubjectPublicKeyInfo` structure, then `parsePublicKey` for the algorithms Go knows.
fn parse_spki<'a>(tbs: &mut Der<'a>) -> Result<Spki<'a>, String> {
    let raw = tbs.read_element(SEQUENCE).ok_or("x509: malformed spki")?;
    let mut spki = Der(Der(raw).read(SEQUENCE).ok_or("x509: malformed spki")?);
    let algorithm = spki
        .read(SEQUENCE)
        .ok_or("x509: malformed public key algorithm identifier")?;
    let algorithm = parse_algorithm(algorithm)?;
    let bits = spki
        .read_bit_string()
        .ok_or("x509: malformed subjectPublicKey")?
        .right_align();
    let (key, rsa) = parse_public_key(algorithm, &bits)?;
    Ok(Spki {
        raw,
        bits,
        key,
        rsa,
    })
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
            parse_san(ext.value, cert)?;
            cert.san = Some(ext.value.to_vec());
            // Go: a subject alternative name extension without a name it reads is unhandled.
            return Ok(cert.dns_names.is_empty()
                && cert.email_addresses.is_empty()
                && cert.ip_addresses.is_empty()
                && cert.uris.is_empty());
        }
        30 => return parse_name_constraints(ext.value, cert),
        31 => check_crl_distribution_points(ext.value)?,
        35 => cert.authority_key_id = parse_authority_key_id(ext)?,
        36 => parse_policy_constraints(ext.value, cert)?,
        37 => cert.server_auth = parse_ext_key_usage(ext.value)?,
        14 => cert.subject_key_id = parse_subject_key_id(ext)?,
        32 => cert.policies = parse_policies(ext.value)?,
        33 => cert.policy_mappings = parse_policy_mappings(ext.value)?,
        54 => {
            cert.inhibit_any_policy = Some(
                Der(ext.value)
                    .read_int(INTEGER)
                    .ok_or("x509: invalid inhibit any policy extension")?,
            );
        }
        _ => return Ok(true),
    }
    Ok(false)
}

/// Go: `parseKeyUsageExtension`: the first nine bits.
fn parse_key_usage(value: &[u8]) -> Result<u16, String> {
    let bits = Der(value)
        .read_bit_string()
        .ok_or("x509: invalid key usage")?
        .bytes;
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

/// An IA5 value as a Go string (the caller has checked it is ASCII).
fn ascii(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

/// Go: `parseSANExtension` (`forEachSAN`): the DNS, email, IP and URI names, in order.
fn parse_san(value: &[u8], cert: &mut Certificate) -> Result<(), String> {
    let mut list = Der(Der(value)
        .read(SEQUENCE)
        .ok_or("x509: invalid subject alternative names")?);
    while !list.is_empty() {
        let name = list
            .read_any()
            .ok_or("x509: invalid subject alternative name")?;
        match name.tag ^ 0x80 {
            1 if is_ia5(name.contents) => cert.email_addresses.push(ascii(name.contents)),
            1 => return Err("x509: SAN rfc822Name is malformed".to_owned()),
            2 if is_ia5(name.contents) => cert.dns_names.push(ascii(name.contents)),
            2 => return Err("x509: SAN dNSName is malformed".to_owned()),
            6 if is_ia5(name.contents) => cert.uris.push(parse_uri(&ascii(name.contents))?),
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
            }
            _ => {}
        }
    }
    Ok(())
}

/// Go: a URI name of `parseSANExtension`: `url.Parse`, then a host must be a valid domain.
fn parse_uri(text: &str) -> Result<Url, String> {
    let uri =
        Url::parse(text).map_err(|e| format!("x509: cannot parse URI {}: {e}", quote(text)))?;
    if !uri.host.is_empty() && !domain_name_valid(&uri.host, false) {
        return Err(format!(
            "x509: cannot parse URI {}: invalid domain",
            quote(text)
        ));
    }
    Ok(uri)
}

/// Go: `parseNameConstraintsExtension`; reports a name type Go does not handle.
fn parse_name_constraints(value: &[u8], cert: &mut Certificate) -> Result<bool, String> {
    let mut outer = Der(value);
    let mut top = Der(outer.read(SEQUENCE).ok_or(INVALID_NAME_CONSTRAINTS)?);
    let permitted = top.read_optional(CONTEXT_0_CONSTRUCTED, INVALID_NAME_CONSTRAINTS)?;
    let excluded = top.read_optional(CONTEXT_1_CONSTRUCTED, INVALID_NAME_CONSTRAINTS)?;
    if !outer.is_empty() || !top.is_empty() {
        return Err(INVALID_NAME_CONSTRAINTS.to_owned());
    }
    let permitted = permitted.unwrap_or_default();
    let excluded = excluded.unwrap_or_default();
    if permitted.is_empty() && excluded.is_empty() {
        return Err("x509: empty name constraints extension".to_owned());
    }
    let mut unhandled = false;
    let constraints = NameConstraints {
        permitted: parse_subtrees(permitted, &mut unhandled)?,
        excluded: parse_subtrees(excluded, &mut unhandled)?,
    };
    cert.name_constraints = Some(constraints);
    Ok(unhandled)
}

/// Go: an IA5 constraint value, or `isIA5String`'s error.
fn ia5_constraint(value: &[u8]) -> Result<String, String> {
    if is_ia5(value) {
        Ok(ascii(value))
    } else {
        Err(format!(
            "x509: invalid constraint value: x509: {} cannot be encoded as an IA5String",
            quote_bytes(value)
        ))
    }
}

/// Go: `isValidIPMask`: ones, then zeros.
fn valid_ip_mask(mask: &[u8]) -> bool {
    let mut seen_zero = false;
    mask.iter().all(|&b| {
        if seen_zero {
            return b == 0;
        }
        match b {
            0x00 | 0x80 | 0xC0 | 0xE0 | 0xF0 | 0xF8 | 0xFC | 0xFE => {
                seen_zero = true;
                true
            }
            0xFF => true,
            _ => false,
        }
    })
}

/// Go: `getValues` of `parseNameConstraintsExtension`: each subtree's base name, checked as Go
/// checks it; other name types set `unhandled`.
fn parse_subtrees(der: &[u8], unhandled: &mut bool) -> Result<Subtrees, String> {
    let mut list = Der(der);
    let mut out = Subtrees::default();
    while !list.is_empty() {
        let base = Der(list.read(SEQUENCE).ok_or(INVALID_NAME_CONSTRAINTS)?)
            .read_any()
            .ok_or(INVALID_NAME_CONSTRAINTS)?;
        match base.tag {
            // dNSName
            0x82 => {
                let domain = ia5_constraint(base.contents)?;
                if !domain_name_valid(&domain, true) {
                    return Err(format!(
                        "x509: failed to parse dnsName constraint {}",
                        quote(&domain)
                    ));
                }
                out.dns.push(domain);
            }
            // iPAddress
            0x87 => {
                let half = match base.contents.len() {
                    8 => 4,
                    32 => 16,
                    len => {
                        return Err(format!(
                            "x509: IP constraint contained value of length {len}"
                        ));
                    }
                };
                let (ip, mask) = base.contents.split_at(half);
                if !valid_ip_mask(mask) {
                    return Err(format!(
                        "x509: IP constraint contained invalid mask {}",
                        hex(mask)
                    ));
                }
                out.ips.push(IpNet {
                    ip: ip.to_vec(),
                    mask: mask.to_vec(),
                });
            }
            // rfc822Name
            0x81 => {
                let constraint = ia5_constraint(base.contents)?;
                let valid = if constraint.contains('@') {
                    parse_mailbox(&constraint).is_some()
                } else {
                    domain_name_valid(&constraint, true)
                };
                if !valid {
                    return Err(format!(
                        "x509: failed to parse rfc822Name constraint {}",
                        quote(&constraint)
                    ));
                }
                out.emails.push(constraint);
            }
            // uniformResourceIdentifier
            0x86 => {
                let domain = ia5_constraint(base.contents)?;
                // Go: net.ParseIP.
                if domain.parse::<IpAddr>().is_ok() {
                    return Err(format!(
                        "x509: failed to parse URI constraint {}: cannot be IP address",
                        quote(&domain)
                    ));
                }
                if !domain_name_valid(&domain, true) {
                    return Err(format!(
                        "x509: failed to parse URI constraint {}",
                        quote(&domain)
                    ));
                }
                out.uris.push(domain);
            }
            _ => *unhandled = true,
        }
    }
    Ok(out)
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

/// Go: `parseAuthorityKeyIdentifier`: the key identifier, when there is one.
fn parse_authority_key_id(ext: Extension<'_>) -> Result<Vec<u8>, String> {
    const INVALID: &str = "x509: invalid authority key identifier";
    if ext.critical {
        return Err("x509: authority key identifier incorrectly marked critical".to_owned());
    }
    let mut akid = Der(Der(ext.value).read(SEQUENCE).ok_or(INVALID)?);
    if akid.peek_tag() == Some(CONTEXT_0) {
        return Ok(akid.read(CONTEXT_0).ok_or(INVALID)?.to_vec());
    }
    Ok(Vec::new())
}

/// Go: the subject key identifier case of `processExtensions`.
fn parse_subject_key_id(ext: Extension<'_>) -> Result<Vec<u8>, String> {
    if ext.critical {
        return Err("x509: subject key identifier incorrectly marked critical".to_owned());
    }
    Der(ext.value)
        .read(OCTET_STRING)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "x509: invalid subject key identifier".to_owned())
}

/// Go: the policy constraints case of `processExtensions`: `requireExplicitPolicy` (`[0]`),
/// then `inhibitPolicyMapping` (`[1]`).
fn parse_policy_constraints(value: &[u8], cert: &mut Certificate) -> Result<(), String> {
    let mut seq = Der(Der(value)
        .read(SEQUENCE)
        .ok_or(INVALID_POLICY_CONSTRAINTS)?);
    if seq.peek_tag() == Some(CONTEXT_0) {
        cert.require_explicit_policy =
            Some(seq.read_int(CONTEXT_0).ok_or(INVALID_POLICY_CONSTRAINTS)?);
    }
    if seq.peek_tag() == Some(CONTEXT_1) {
        cert.inhibit_policy_mapping =
            Some(seq.read_int(CONTEXT_1).ok_or(INVALID_POLICY_CONSTRAINTS)?);
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
fn parse_policies(value: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut list = Der(Der(value).read(SEQUENCE).ok_or(INVALID_POLICIES)?);
    let mut policies: Vec<Vec<u8>> = Vec::new();
    while !list.is_empty() {
        let oid = Der(list.read(SEQUENCE).ok_or(INVALID_POLICIES)?)
            .read(OBJECT_IDENTIFIER)
            .ok_or(INVALID_POLICIES)?;
        if policies.iter().any(|p| p == oid) || !oid_der_valid(oid) {
            return Err(INVALID_POLICIES.to_owned());
        }
        policies.push(oid.to_vec());
    }
    Ok(policies)
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

/// Go: the policy mappings case of `processExtensions`: issuer and subject domain policies, as
/// the OIDs' contents (Go does not check them).
fn parse_policy_mappings(value: &[u8]) -> Result<Vec<PolicyMapping>, String> {
    let mut list = Der(Der(value).read(SEQUENCE).ok_or(INVALID_POLICY_MAPPINGS)?);
    let mut mappings = Vec::new();
    while !list.is_empty() {
        let mut mapping = Der(list.read(SEQUENCE).ok_or(INVALID_POLICY_MAPPINGS)?);
        let issuer = mapping
            .read(OBJECT_IDENTIFIER)
            .ok_or(INVALID_POLICY_MAPPINGS)?;
        let subject = mapping
            .read(OBJECT_IDENTIFIER)
            .ok_or(INVALID_POLICY_MAPPINGS)?;
        mappings.push((issuer.to_vec(), subject.to_vec()));
    }
    Ok(mappings)
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
