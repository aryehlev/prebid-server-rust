//! Replays `testdata/vendorlist_goldens.json`, written by `testdata/go/vendorlist.go` from the
//! real `vendorlist2.ParseEagerly` (go-gdpr v1.12.0 on Go's `encoding/json`).
//!
//! The cases are the go-gdpr test lists, realistic IAB-shaped lists, hand-written documents for
//! the `encoding/json` decoding rules, and every prefix, single-byte deletion and many
//! single-byte insertions of a small list, so every scanner error state is reached. Each case
//! records the error text, or the versions and, for every vendor ID in `[0, 65535]` that is
//! present, the ids in `[0, 255]` each accessor returns true for.

use serde_json::{Map, Value, json};

use super::parse_eagerly;
use crate::{
    api::{Vendor, VendorList as _},
    consentconstants::{Purpose, SpecialFeature},
    test_support::{TestError, TestResult, input_bytes, load_cases, put},
};

/// Go: the `[]int` of ids in `[0, 255]` for which `f` is true; `null` when empty, since the Go
/// slice is then nil.
fn ids<F: Fn(u8) -> bool>(f: F) -> Value {
    let ids: Vec<u8> = (0..=u8::MAX).filter(|&id| f(id)).collect();
    if ids.is_empty() {
        Value::Null
    } else {
        json!(ids)
    }
}

/// Go: `vendorDump`.
fn vendor_dump(id: u16, vendor: &dyn Vendor) -> Value {
    json!({
        "id": id,
        "purposes": ids(|p| vendor.purpose(Purpose(p))),
        "purposes_strict": ids(|p| vendor.purpose_strict(Purpose(p))),
        "li": ids(|p| vendor.legitimate_interest(Purpose(p))),
        "li_strict": ids(|p| vendor.legitimate_interest_strict(Purpose(p))),
        "special_purposes": ids(|p| vendor.special_purpose(Purpose(p))),
        "special_features": ids(|p| vendor.special_feature(SpecialFeature(p))),
    })
}

/// Go: `newVendorListCase`.
fn record_case(case: &Value) -> Result<Value, TestError> {
    let input = input_bytes(case)?;
    let mut record = Map::new();
    for key in ["name", "input", "input_hex"] {
        if let Some(value) = case.get(key) {
            put(&mut record, key, value.clone());
        }
    }
    let (spec_version, version) = match parse_eagerly(&input) {
        Err(err) => {
            put(&mut record, "err", json!(err.to_string()));
            (0, 0)
        }
        Ok(list) => {
            let vendors: Vec<Value> = (0..=u16::MAX)
                .filter_map(|id| list.vendor(id).map(|vendor| vendor_dump(id, vendor)))
                .collect();
            if !vendors.is_empty() {
                put(&mut record, "vendors", json!(vendors));
            }
            (list.spec_version(), list.version())
        }
    };
    put(&mut record, "spec_version", json!(spec_version));
    put(&mut record, "version", json!(version));
    Ok(Value::Object(record))
}

/// Replays every `part`-th of `PARTS` cases, so the replay runs on several test threads.
fn replay(part: usize) -> TestResult {
    const PARTS: usize = 4;
    let cases = load_cases("vendorlist_goldens.json")?;
    check!(cases.len() > 2800, "only {} vendor list cases", cases.len());
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
    replay(0)
}

#[test]
fn go_goldens_part1() -> TestResult {
    replay(1)
}

#[test]
fn go_goldens_part2() -> TestResult {
    replay(2)
}

#[test]
fn go_goldens_part3() -> TestResult {
    replay(3)
}
