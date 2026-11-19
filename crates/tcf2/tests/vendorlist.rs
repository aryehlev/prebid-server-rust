//! Pins of the global vendor list read on serde (`gvl.json-leniency` in the differences ledger)
//! against go-gdpr v1.12.0's `vendorlist2.ParseEagerly`, which reads it with `encoding/json`
//! (go1.26.2): every GVL fixture of go-gdpr and prebid-server reads as in Go
//! (`testdata/gvl_fixture_goldens.json`), and each document of `testdata/vendorlist_goldens.json`
//! and `testdata/vendorlist_fuzz_goldens.json` (hand-written `encoding/json` cases, edits of a
//! small list, and grammar-generated documents varying types, nulls, duplicates, key spelling
//! and escapes) reads as in Go or otherwise for a reason the ledger records. `testdata/go` writes
//! all three.

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, error::Error, fmt::Debug, fs, path::Path, thread};

    use pbs_tcf2::{
        api::{Vendor, VendorList as _},
        consentconstants::{Purpose, SpecialFeature},
        vendorlist2::parse_eagerly,
    };
    use serde_json::{Map, Value, json};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Fails the test with `message` unless `left == right`.
    fn check_eq<T: PartialEq + Debug + ?Sized>(left: &T, right: &T, message: &str) -> TestResult {
        if left == right {
            Ok(())
        } else {
            Err(format!("{message}: {left:?} != {right:?}").into())
        }
    }

    /// The `cases` of `testdata/<name>`, written by go-gdpr v1.12.0's generator on go1.26.2.
    fn cases(name: &str) -> Result<Vec<Value>, Box<dyn Error>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name);
        let mut doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
        for (pointer, want) in [
            ("/header/go_gdpr_version", "v1.12.0"),
            ("/header/go_version", "go1.26.2"),
        ] {
            check_eq(
                &doc.pointer(pointer).and_then(Value::as_str),
                &Some(want),
                name,
            )?;
        }
        match doc.get_mut("cases").map(Value::take) {
            Some(Value::Array(cases)) => Ok(cases),
            _ => Err(format!("{name} has no cases").into()),
        }
    }

    /// The bytes a lower-case hex string spells.
    fn unhex(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let (pairs, rest) = text.as_bytes().as_chunks::<2>();
        if !rest.is_empty() {
            return Err(format!("odd-length hex {text:?}").into());
        }
        pairs
            .iter()
            .map(|pair| Ok(u8::from_str_radix(str::from_utf8(pair)?, 16)?))
            .collect()
    }

    /// The input of a golden case: `input` (valid UTF-8) or `input_hex`.
    fn input(case: &Value) -> Result<Vec<u8>, Box<dyn Error>> {
        match (case.get("input"), case.get("input_hex")) {
            (Some(Value::String(text)), _) => Ok(text.as_bytes().to_vec()),
            (_, Some(Value::String(hex))) => unhex(hex),
            _ => Err(format!("case without input: {case}").into()),
        }
    }

    /// The ids in `[0, 255]` for which `f` is true, `null` when there are none (Go's nil slice).
    fn ids<F: Fn(u8) -> bool>(f: F) -> Value {
        let ids: Vec<u8> = (0..=u8::MAX).filter(|&id| f(id)).collect();
        if ids.is_empty() {
            Value::Null
        } else {
            json!(ids)
        }
    }

    /// The generator's `vendorDump` of one vendor.
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

    /// The generator's `newVendorListCase` record of `case`, built from `parse_eagerly`: the error
    /// text, or the versions and every vendor id in `[0, 65535]` that is present.
    fn record(case: &Value) -> Result<Value, Box<dyn Error>> {
        let mut record = Map::new();
        for key in ["name", "input", "input_hex"] {
            if let Some(value) = case.get(key) {
                let _previous = record.insert(key.to_owned(), value.clone());
            }
        }
        let (spec_version, version) = match parse_eagerly(&input(case)?) {
            Err(err) => {
                let _previous = record.insert("err".to_owned(), json!(err.to_string()));
                (0, 0)
            }
            Ok(list) => {
                let vendors: Vec<Value> = (0..=u16::MAX)
                    .filter_map(|id| list.vendor(id).map(|vendor| vendor_dump(id, vendor)))
                    .collect();
                if !vendors.is_empty() {
                    let _previous = record.insert("vendors".to_owned(), json!(vendors));
                }
                (list.spec_version(), list.version())
            }
        };
        for (key, value) in [("spec_version", spec_version), ("version", version)] {
            let _previous = record.insert(key.to_owned(), json!(value));
        }
        Ok(Value::Object(record))
    }

    /// A record in a line: its error, or its versions and vendor ids.
    fn summary(record: &Value) -> String {
        if let Some(err) = record.get("err").and_then(Value::as_str) {
            return err.to_owned();
        }
        let vendors: Vec<String> =
            record
                .get("vendors")
                .and_then(Value::as_array)
                .map_or_else(Vec::new, |vendors| {
                    vendors
                        .iter()
                        .filter_map(|vendor| vendor.get("id").map(ToString::to_string))
                        .collect()
                });
        format!(
            "spec {} version {} vendors [{}]",
            record.get("spec_version").unwrap_or(&Value::Null),
            record.get("version").unwrap_or(&Value::Null),
            vendors.join(" ")
        )
    }

    /// Every GVL fixture of go-gdpr v1.12.0 (the `testDataSpecVersion2`, `testDataSpecVersion3`
    /// lists and their empty forms) and of prebid-server (gdpr/ and its tests) reads to Go's
    /// versions, vendors and accessor results. The one malformed fixture is refused by both, each
    /// with its own library's text.
    // Go: testdata/go/gvlfixtures.go (buildGVLFixtureCases), through vendorlist2.ParseEagerly
    // (vendorlist2/eager-parsing.go:19-40).
    #[test]
    fn gvl_fixtures() -> TestResult {
        let cases = cases("gvl_fixture_goldens.json")?;
        check_eq(&cases.len(), &34, "fixtures")?;
        let mut refused = Vec::new();
        for case in &cases {
            let name = case.get("name").and_then(Value::as_str).unwrap_or_default();
            let mut rust = record(case)?;
            match (case.get("err"), rust.as_object_mut()) {
                (Some(go_err), Some(fields)) => {
                    let rust_err = fields.remove("err").unwrap_or_default();
                    refused.push((name.to_owned(), go_err.clone(), rust_err));
                    let mut go = case.clone();
                    let _go_err = go.as_object_mut().and_then(|go| go.remove("err"));
                    check_eq(&rust, &go, name)?;
                }
                _ => check_eq(&rust, case, name)?,
            }
        }
        check_eq(
            refused.as_slice(),
            &[(
                "prebid-server TestMalformedVendorlist".to_owned(),
                json!("invalid character 'm' looking for beginning of value"),
                json!("expected value at line 1 column 1"),
            )][..],
            "refused fixtures: Go's text, then serde_json's",
        )
    }

    /// Why a document of the corpora reads as it does beside Go: the ledger entry that says so.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Reason {
        /// The record is Go's.
        Same,
        /// Both refuse the document, each with its own library's text (`gvl.json-leniency`).
        ErrorText,
        /// serde refuses a member a document repeats, which Go reads again (`json.duplicate-keys`).
        DuplicateKey,
        /// serde refuses a string where a list of ids belongs, which Go decodes as standard
        /// base64 (`gvl.json-leniency`).
        Base64Ids,
        /// serde refuses a member name or vendor key that is not UTF-8, which Go reads with
        /// U+FFFD (`json.invalid-utf8`).
        InvalidUtf8Key,
        /// serde refuses a member name or vendor key with a lone surrogate escape, which Go reads
        /// as U+FFFD (`json.lone-surrogates`).
        LoneSurrogateKey,
        /// Go reads a member whose name differs in case, serde skips it (`json.key-case`).
        FoldedKey,
        /// Go refuses a skipped member nested past 10000 levels, serde reads past it
        /// (`json.nesting-depth`).
        DeepNesting,
        /// serde reads an array of a contract's members, in order, as the contract, which Go
        /// refuses (`gvl.json-leniency`).
        ArrayAsContract,
    }

    /// The members of go-gdpr's `vendorListContract` and `vendorListVendorContract`.
    const MEMBERS: [&str; 9] = [
        "gvlSpecificationVersion",
        "vendorListVersion",
        "vendors",
        "id",
        "purposes",
        "legIntPurposes",
        "flexiblePurposes",
        "specialPurposes",
        "specialFeatures",
    ];

    /// A name as `encoding/json` folds it to match a member: ASCII upper-cased, `ſ` (U+017F) as
    /// `S` and the Kelvin sign as `K`; no other character folds to ASCII.
    // Go: $GOROOT/src/encoding/json/fold.go (foldName, foldRune).
    fn fold(name: &str) -> String {
        name.chars()
            .map(|c| match c {
                '\u{17F}' => 'S',
                '\u{212A}' => 'K',
                other => other.to_ascii_uppercase(),
            })
            .collect()
    }

    /// The object keys of `input`, read leniently (every string followed by a `:`), unescaped
    /// where `serde_json` can unescape them.
    fn keys(input: &[u8]) -> Vec<String> {
        let mut keys = Vec::new();
        let mut rest = input;
        while let Some(open) = rest.iter().position(|&b| b == b'"') {
            rest = rest.get(open + 1..).unwrap_or_default();
            let mut escaped = false;
            let Some(close) = rest.iter().position(|&b| {
                let close = !escaped && b == b'"';
                escaped = !escaped && b == b'\\';
                close
            }) else {
                break;
            };
            let raw = rest.get(..close).unwrap_or_default();
            rest = rest.get(close + 1..).unwrap_or_default();
            let next = rest.iter().find(|b| !b" \t\r\n".contains(b));
            if next == Some(&b':') {
                let quoted = [&b"\""[..], raw, b"\""].concat();
                keys.push(
                    serde_json::from_slice(&quoted)
                        .unwrap_or_else(|_| String::from_utf8_lossy(raw).into_owned()),
                );
            }
        }
        keys
    }

    /// Whether `input` names a member in another spelling that Go folds to it.
    fn folded_member(input: &[u8]) -> bool {
        keys(input).iter().any(|key| {
            MEMBERS
                .iter()
                .any(|member| member != key && fold(member) == fold(key))
        })
    }

    /// Why serde refused a document Go reads, from `serde_json`'s text.
    fn serde_refusal(err: &str) -> Option<Reason> {
        if err.starts_with("duplicate field `") {
            Some(Reason::DuplicateKey)
        } else if err.starts_with("invalid type: string ") && err.contains(", expected a sequence")
        {
            Some(Reason::Base64Ids)
        } else if err.starts_with("invalid unicode code point at ") {
            Some(Reason::InvalidUtf8Key)
        } else if err.starts_with("unexpected end of hex escape at ")
            || err.starts_with("lone leading surrogate in hex escape at ")
        {
            Some(Reason::LoneSurrogateKey)
        } else {
            None
        }
    }

    /// Why Go refused a document serde reads, from `encoding/json`'s text.
    fn go_refusal(err: &str) -> Option<Reason> {
        if err.ends_with(" exceeded max depth") {
            Some(Reason::DeepNesting)
        } else if err.starts_with("json: cannot unmarshal array into Go ")
            && (err.ends_with(" of type vendorlist2.vendorListContract")
                || err.ends_with(" of type vendorlist2.vendorListVendorContract"))
        {
            Some(Reason::ArrayAsContract)
        } else {
            None
        }
    }

    /// The reason for `rust`, the record built here for Go's `case` of `input`, if one applies.
    fn reason(case: &Value, input: &[u8], rust: &Value) -> Option<Reason> {
        if rust == case {
            return Some(Reason::Same);
        }
        let folded = || folded_member(input).then_some(Reason::FoldedKey);
        match (
            case.get("err").and_then(Value::as_str),
            rust.get("err").and_then(Value::as_str),
        ) {
            (Some(_), Some(_)) => Some(Reason::ErrorText),
            (None, Some(err)) => serde_refusal(err).or_else(folded),
            (Some(err), None) => go_refusal(err).or_else(folded),
            (None, None) => folded(),
        }
    }

    /// One case of a corpus: its name, its reason, Go's record and the one built here.
    struct Outcome {
        name: String,
        reason: Reason,
        go: Value,
        rust: Value,
    }

    /// Reads `case` here and finds the reason for what it gives.
    fn outcome(file: &str, case: &Value) -> Result<Outcome, String> {
        let name = case.get("name").and_then(Value::as_str).unwrap_or_default();
        let input = input(case).map_err(|e| format!("{file}: {name}: {e}"))?;
        let rust = record(case).map_err(|e| format!("{file}: {name}: {e}"))?;
        let reason = reason(case, &input, &rust)
            .ok_or_else(|| format!("{file}: {name}: unexplained: Go {case}, Rust {rust}"))?;
        Ok(Outcome {
            name: name.to_owned(),
            reason,
            go: case.clone(),
            rust,
        })
    }

    /// The outcome of every case of `file`, on four threads: each case looks up all 65,536 vendor
    /// ids.
    fn outcomes(file: &str) -> Result<Vec<Outcome>, Box<dyn Error>> {
        let cases = cases(file)?;
        let chunk = cases.len().div_ceil(4).max(1);
        thread::scope(|scope| {
            let workers: Vec<_> = cases
                .chunks(chunk)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|case| outcome(file, case))
                            .collect::<Result<Vec<_>, _>>()
                    })
                })
                .collect();
            let mut all = Vec::with_capacity(cases.len());
            for worker in workers {
                let done = worker
                    .join()
                    .map_err(|_panic| format!("{file}: a worker panicked"))?;
                all.extend(done?);
            }
            Ok(all)
        })
    }

    /// How many documents of each corpus read for each reason.
    const COUNTS: [(&str, &[(Reason, usize)]); 2] = [
        (
            "vendorlist_goldens.json",
            &[
                (Reason::Same, 1184),
                (Reason::ErrorText, 1663),
                (Reason::DuplicateKey, 10),
                (Reason::Base64Ids, 6),
                (Reason::InvalidUtf8Key, 38),
                (Reason::LoneSurrogateKey, 1),
                (Reason::FoldedKey, 5),
                (Reason::DeepNesting, 2),
                (Reason::ArrayAsContract, 2),
            ],
        ),
        (
            "vendorlist_fuzz_goldens.json",
            &[
                (Reason::Same, 229),
                (Reason::ErrorText, 715),
                (Reason::DuplicateKey, 792),
                (Reason::Base64Ids, 211),
                (Reason::InvalidUtf8Key, 5),
                (Reason::FoldedKey, 154),
            ],
        ),
    ];

    /// One document of `vendorlist_goldens.json` for each reason: its name, its reason, and Go's
    /// record and serde's in a line each.
    const EXAMPLES: [(&str, Reason, &str, &str); 11] = [
        (
            "version only",
            Reason::Same,
            "spec 0 version 1 vendors []",
            "spec 0 version 1 vendors []",
        ),
        (
            "vendor null",
            Reason::Same,
            "spec 0 version 7 vendors [0]",
            "spec 0 version 7 vendors [0]",
        ),
        (
            "trailing garbage",
            Reason::ErrorText,
            "invalid character 'x' after top-level value",
            "trailing characters at line 1 column 24",
        ),
        (
            "version duplicate",
            Reason::DuplicateKey,
            "spec 0 version 4 vendors []",
            "duplicate field `vendorListVersion` at line 1 column 42",
        ),
        (
            "purposes base64 string",
            Reason::Base64Ids,
            "spec 0 version 7 vendors [1]",
            r#"invalid type: string "AQID", expected a sequence at line 1 column 63"#,
        ),
        (
            "vendor key invalid utf8 collide",
            Reason::InvalidUtf8Key,
            "spec 0 version 7 vendors [2]",
            "invalid unicode code point at line 1 column 36",
        ),
        (
            "escaped vendor keys collide",
            Reason::LoneSurrogateKey,
            "spec 0 version 7 vendors [3]",
            "unexpected end of hex escape at line 1 column 42",
        ),
        (
            "case insensitive keys",
            Reason::FoldedKey,
            "spec 3 version 7 vendors [5]",
            "data.vendorListVersion was 0 or undefined. Versions should start at 1",
        ),
        (
            "nesting depth 10001 arrays",
            Reason::DeepNesting,
            "invalid character '[' exceeded max depth",
            "spec 0 version 1 vendors []",
        ),
        (
            "array of the members",
            Reason::ArrayAsContract,
            "json: cannot unmarshal array into Go value of type vendorlist2.vendorListContract",
            "spec 3 version 7 vendors [5]",
        ),
        (
            "vendor array of the members",
            Reason::ArrayAsContract,
            "json: cannot unmarshal array into Go struct field vendorListContract.vendors of \
             type vendorlist2.vendorListVendorContract",
            "spec 0 version 7 vendors [5]",
        ),
    ];

    /// Each document of the two corpora reads as in Go, or otherwise for one of the reasons the
    /// ledger records, counted per reason; one named document of each reason shows both sides.
    // Go: testdata/go/vendorlist.go (buildVendorListCases) and testdata/go/fuzz.go
    // (buildFuzzVendorListCases), through vendorlist2.ParseEagerly.
    #[test]
    fn gvl_corpus() -> TestResult {
        let mut main = Vec::new();
        for (file, want) in COUNTS {
            let found = outcomes(file)?;
            let mut counts: BTreeMap<Reason, usize> = BTreeMap::new();
            for case in &found {
                *counts.entry(case.reason).or_default() += 1;
            }
            check_eq(
                counts.into_iter().collect::<Vec<_>>().as_slice(),
                want,
                file,
            )?;
            if main.is_empty() {
                main = found;
            }
        }

        let got = EXAMPLES
            .iter()
            .map(|&(name, ..)| {
                let case = main
                    .iter()
                    .find(|case| case.name == name)
                    .ok_or_else(|| format!("no case {name:?}"))?;
                Ok((name, case.reason, summary(&case.go), summary(&case.rust)))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let want: Vec<_> = EXAMPLES
            .iter()
            .map(|&(name, reason, go, rust)| (name, reason, go.to_owned(), rust.to_owned()))
            .collect();
        check_eq(&got, &want, "side by side")
    }
}
