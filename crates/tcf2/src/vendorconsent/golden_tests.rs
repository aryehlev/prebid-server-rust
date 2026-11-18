//! Replays `testdata/consent_goldens.json`, written by `testdata/go/consent.go` from the real
//! go-gdpr v1.12.0.
//!
//! For every input string the generator records `vendorconsent.ParseString`, `tcf2.ParseString`
//! (for strings not starting with `C`), `base64.RawURLEncoding.DecodeString` and
//! `vendorconsent.Parse` of the decoded bytes, with every accessor result: the header fields, all
//! purposes, every vendor ID in `[0, 65535]` for consent and legitimate interest, the section
//! encodings, the parsed publisher restrictions, and `CheckPubRestriction` probes (including the
//! `uint8` wrap-around of the lookup key). This builds the same JSON from the Rust results and
//! compares the two documents.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::{
    ParsedConsent, parse, parse_string, tcf1, tcf2,
    tcf2::{ConsentMetadata, VendorSection},
};
use crate::{
    ConsentError,
    api::VendorConsents,
    base64::RAW_URL_ENCODING,
    consentconstants::Purpose,
    test_support::{TestError, TestResult, hex_encode, input_bytes, load_cases, put},
};

/// Go: `wrapPurposes`, `wrapTypes` and `wrapVendorSamples` in `consent.go`.
const WRAP_PURPOSES: [u8; 8] = [64, 65, 66, 100, 128, 129, 200, 255];
const WRAP_TYPES: [u8; 8] = [0, 1, 2, 3, 4, 5, 7, 255];
const WRAP_VENDOR_SAMPLES: usize = 4;

/// Go: `defaultPubSamples` in `consent.go`.
const DEFAULT_PUB_SAMPLES: [u16; 8] = [0, 1, 2, 3, 32766, 32767, 32768, 65535];

const fn bit(value: bool) -> char {
    if value { '1' } else { '0' }
}

/// Go: `idRanges`: the ids in `[0, 65535]` for which `f` is true, as `a-b,c,...`.
fn id_ranges<F: Fn(u16) -> bool>(f: F) -> String {
    fn push(out: &mut String, start: u16, end: u16) {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&start.to_string());
        if start != end {
            out.push('-');
            out.push_str(&end.to_string());
        }
    }
    let mut out = String::new();
    let mut start = None;
    for id in 0..=u16::MAX {
        if f(id) {
            start = start.or(Some(id));
        } else if let Some(first) = start.take() {
            push(&mut out, first, id - 1);
        }
    }
    if let Some(first) = start {
        push(&mut out, first, u16::MAX);
    }
    out
}

/// Go: `commonMeta`. `purpose` is the checked `PurposeAllowed`, probed for ids below `limit`
/// until the first Go panic.
fn common_meta<C: VendorConsents>(
    consent: &C,
    purpose: &dyn Fn(Purpose) -> Result<bool, ConsentError>,
    limit: u16,
) -> Map<String, Value> {
    let mut purposes = String::new();
    let mut panic_from = 256;
    for id in 0..limit {
        let Ok(id) = u8::try_from(id) else { break };
        let Ok(allowed) = purpose(Purpose(id)) else {
            panic_from = id.into();
            break;
        };
        purposes.push(bit(allowed));
    }
    let (created, updated) = (consent.created(), consent.last_updated());
    let mut meta = Map::new();
    for (key, value) in [
        ("version", json!(consent.version())),
        ("created", json!([created.unix(), created.nanosecond()])),
        (
            "last_updated",
            json!([updated.unix(), updated.nanosecond()]),
        ),
        ("cmp_id", json!(consent.cmp_id())),
        ("cmp_version", json!(consent.cmp_version())),
        ("consent_screen", json!(consent.consent_screen())),
        (
            "consent_language_hex",
            json!(hex_encode(&consent.consent_language())),
        ),
        ("vendor_list_version", json!(consent.vendor_list_version())),
        ("tcf_policy_version", json!(consent.tcf_policy_version())),
        ("max_vendor_id", json!(consent.max_vendor_id())),
        ("purpose_allowed", json!(purposes)),
        ("purpose_panic_from", json!(panic_from)),
        (
            "vendor_consent",
            json!(id_ranges(|id| consent.vendor_consent(id))),
        ),
    ] {
        put(&mut meta, key.to_owned(), value);
    }
    meta
}

const fn encoding_name(section: &VendorSection) -> &'static str {
    match section {
        VendorSection::BitField { .. } => "bitfield",
        VendorSection::Range(_) => "range",
    }
}

/// Go: `addTCF2Meta`.
fn add_tcf2_meta(
    meta: &mut Map<String, Value>,
    consent: &ConsentMetadata,
) -> Result<(), TestError> {
    let li: String = (0..32)
        .map(|id| bit(consent.purpose_li_transparency(Purpose(id))))
        .collect();
    let features: String = (0..16)
        .map(|id| bit(consent.special_feature_opt_in(id)))
        .collect();
    for (key, value) in [
        ("purpose_li", json!(li)),
        ("special_feature_opt_in", json!(features)),
        (
            "purpose_one_treatment",
            json!(consent.purpose_one_treatment()),
        ),
        (
            "vendor_li_max_id",
            json!(consent.vendor_legit_interest_max_id()),
        ),
        (
            "vendor_li",
            json!(id_ranges(|id| consent.vendor_legit_interest(id))),
        ),
        (
            "consent_encoding",
            json!(encoding_name(&consent.vendor_consents)),
        ),
        (
            "li_encoding",
            json!(encoding_name(&consent.vendor_legitimate_interests)),
        ),
    ] {
        put(meta, key.to_owned(), value);
    }

    let restrictions = &consent.publisher_restrictions.restrictions;
    if restrictions.is_empty() {
        // Go checks this itself and records nothing.
        for key in 0..=u8::MAX {
            for vendor in DEFAULT_PUB_SAMPLES {
                if consent.check_pub_restriction(key >> 2, key & 3, vendor) {
                    return Err(format!("restriction without dump for key {key}").into());
                }
            }
        }
        return Ok(());
    }

    let mut samples: BTreeSet<u16> = DEFAULT_PUB_SAMPLES.into_iter().collect();
    let mut dump = Vec::new();
    for (&key, vendors) in restrictions {
        let mut ranges = Vec::new();
        for entry in vendors {
            ranges.push(json!([entry.start_id, entry.end_id]));
            for endpoint in [entry.start_id, entry.end_id] {
                samples.extend(
                    [
                        endpoint.checked_sub(1),
                        Some(endpoint),
                        endpoint.checked_add(1),
                    ]
                    .into_iter()
                    .flatten(),
                );
            }
        }
        dump.push(json!({
            "key": key,
            "purpose": (key & 0xfc) >> 2,
            "type": key & 0x03,
            "vendors": ranges,
        }));
    }
    let samples: Vec<u16> = samples.into_iter().collect();

    let mut checks = Map::new();
    for key in 0..=u8::MAX {
        let probes: String = samples
            .iter()
            .map(|&vendor| bit(consent.check_pub_restriction(key >> 2, key & 3, vendor)))
            .collect();
        if probes.contains('1') {
            put(&mut checks, key.to_string(), json!(probes));
        }
    }

    let mut wrap = String::new();
    for purpose in WRAP_PURPOSES {
        for restrict_type in WRAP_TYPES {
            for &vendor in samples.iter().take(WRAP_VENDOR_SAMPLES) {
                wrap.push(bit(consent.check_pub_restriction(
                    purpose,
                    restrict_type,
                    vendor,
                )));
            }
        }
    }

    put(meta, "pub_dump".to_owned(), json!(dump));
    put(meta, "pub_samples".to_owned(), json!(samples));
    if !checks.is_empty() {
        put(meta, "pub_checks".to_owned(), Value::Object(checks));
    }
    put(meta, "pub_wrap".to_owned(), json!(wrap));
    Ok(())
}

fn error_record(err: &ConsentError) -> Value {
    if err.is_go_panic() {
        json!({ "panic": err.to_string() })
    } else {
        json!({ "err": err.to_string() })
    }
}

fn tcf1_record(result: &Result<tcf1::Consent, ConsentError>) -> Value {
    match result {
        Err(err) => error_record(err),
        Ok(consent) => {
            let kind = match consent.section {
                tcf1::Section::BitField => "tcf1-bitfield",
                tcf1::Section::Range { .. } => "tcf1-range",
            };
            let meta = common_meta(consent, &|id| consent.checked_purpose_allowed(id), 256);
            json!({ "kind": kind, "meta": meta })
        }
    }
}

fn tcf2_record(result: &Result<ConsentMetadata, ConsentError>) -> Result<Value, TestError> {
    match result {
        Err(err) => Ok(error_record(err)),
        Ok(consent) => {
            let mut meta = common_meta(consent, &|id| Ok(consent.purpose_allowed(id)), 32);
            add_tcf2_meta(&mut meta, consent)?;
            Ok(json!({ "kind": "tcf2", "meta": meta }))
        }
    }
}

/// Go: `newCase`.
fn record_case(case: &Value) -> Result<Value, TestError> {
    let input = input_bytes(case)?;
    let mut record = Map::new();
    for key in ["label", "input", "input_hex"] {
        if let Some(value) = case.get(key) {
            put(&mut record, key.to_owned(), value.clone());
        }
    }

    let vendorconsent = match parse_string(&input) {
        Ok(ParsedConsent::Tcf1(consent)) => tcf1_record(&Ok(consent)),
        Ok(ParsedConsent::Tcf2(consent)) => tcf2_record(&Ok(consent))?,
        Err(err) => error_record(&err),
    };
    put(&mut record, "vendorconsent".to_owned(), vendorconsent);
    if !tcf2::is_consent_v2(&input) {
        put(
            &mut record,
            "tcf2".to_owned(),
            tcf2_record(&tcf2::parse_string(&input))?,
        );
    }
    match RAW_URL_ENCODING.decode(&input) {
        Ok(decoded) => {
            put(
                &mut record,
                "raw_url_decode".to_owned(),
                json!({"len": decoded.len()}),
            );
            put(
                &mut record,
                "tcf1_parse".to_owned(),
                tcf1_record(&parse(decoded)),
            );
        }
        Err(err) => {
            put(
                &mut record,
                "raw_url_decode".to_owned(),
                json!({"err": err.to_string(), "len": 0}),
            );
        }
    }
    Ok(Value::Object(record))
}

/// Replays every `part`-th of `PARTS` cases of `file`, so the replay runs on several test
/// threads.
fn replay(file: &str, min_cases: usize, part: usize) -> TestResult {
    const PARTS: usize = 4;
    let cases = load_cases(file)?;
    check!(
        cases.len() > min_cases,
        "only {} cases in {file}",
        cases.len()
    );
    for case in cases.iter().skip(part).step_by(PARTS) {
        let actual = record_case(case)?;
        if &actual != case {
            return Err(format!("mismatch\n  go:   {case}\n  rust: {actual}").into());
        }
    }
    Ok(())
}

#[test]
fn go_goldens_part0() -> TestResult {
    replay("consent_goldens.json", 4000, 0)
}

#[test]
fn go_goldens_part1() -> TestResult {
    replay("consent_goldens.json", 4000, 1)
}

#[test]
fn go_goldens_part2() -> TestResult {
    replay("consent_goldens.json", 4000, 2)
}

#[test]
fn go_goldens_part3() -> TestResult {
    replay("consent_goldens.json", 4000, 3)
}

// `testdata/consent_fuzz_goldens.json` (written by `testdata/go/fuzz.go`): every base64 prefix
// of range-heavy strings, sweeps over the bit alignment of the legitimate-interest header, edited
// section headers, range and publisher restriction shapes, mixed alphabets and segments, and TCF
// 1.1 prefixes.

#[test]
fn go_fuzz_goldens_part0() -> TestResult {
    replay("consent_fuzz_goldens.json", 8000, 0)
}

#[test]
fn go_fuzz_goldens_part1() -> TestResult {
    replay("consent_fuzz_goldens.json", 8000, 1)
}

#[test]
fn go_fuzz_goldens_part2() -> TestResult {
    replay("consent_fuzz_goldens.json", 8000, 2)
}

#[test]
fn go_fuzz_goldens_part3() -> TestResult {
    replay("consent_fuzz_goldens.json", 8000, 3)
}
