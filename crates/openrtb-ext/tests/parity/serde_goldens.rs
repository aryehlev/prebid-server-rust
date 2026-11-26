//! The serde code against the goldens that `golden` replays through `GoJson`.
//!
//! The goldens (`testdata/go/main.go` documents them) are prebid-server's own extension objects
//! (`corpus.tsv`), the generated mutations of every type (`mutations.tsv`) and the edge cases
//! of `review.go` (`review.tsv`); the merges of `review_merges.go` (`review_merges.jsonl`) replay
//! as `shapes` replays its merges. Each row's document is decoded with `pbs_jsonutil::unmarshal`,
//! and through `EncodingJson`, and the value written with `marshal`, against Go's
//! `jsonutil.Unmarshal` and `json.Unmarshal` values written with `jsonutil.Marshal` (the row's
//! first and fourth encodings; one Go wrote as a hash is taken from the `GoJson` port when that
//! matches the hash). Outcomes compare as `shapes` compares them, and every difference must fall in
//! a ledger entry: one the comparison names, one a fix of the document shows (each fix is a
//! ledgered difference: a key respelled as jsoniter matches it, `-0` written `0`, bytes that are
//! not UTF-8 read as U+FFFD), or one what the document holds names where Go's outcome cannot be
//! reproduced (a number jsoniter fails to skip, an unpaired surrogate, a `StringInt` jsonparser
//! reads otherwise).

use std::collections::{BTreeMap, BTreeSet};

use super::{
    golden::{self, Want},
    shapes::{
        self, DUPLICATE_KEYS, ERROR_TEXTS, FAILED_MERGE_ELEMENTS, FLOAT_FORMAT, HTML_ESCAPE,
        KEY_CASE, NIL_RAW_WRITTEN, NULL_DOCUMENT, NULL_ELEMENTS, NUMBER_TEXT, Outcome,
        PARTIAL_RESULTS, PREBID_MESSAGE, STD_ERROR_TEXTS, STRING_OR_ARRAY_NULL, STRUCT_FROM_ARRAY,
    },
    support::{TestResult, ensure_eq, golden_rows, report},
};

/// `json.invalid-utf8`: `serde_json` rejects invalid UTF-8 in a string.
const INVALID_UTF8: &str = "json.invalid-utf8";
/// `json.negative-zero-int`: `serde_json` reads `-0` as a float, which no integer field takes.
const NEGATIVE_ZERO_INT: &str = "json.negative-zero-int";
/// `json.out-of-range-numbers`: `serde_json` skips a number beyond float64's range.
const OUT_OF_RANGE_NUMBERS: &str = "json.out-of-range-numbers";
/// `json.string-int-parse`: `StringInt` reads `"+5"` where Go fails, and fails on `"-"` where Go
/// reads 0.
const STRING_INT_PARSE: &str = "json.string-int-parse";
/// `json.surrogate-pairing`: a high surrogate escape before an escape it does not pair with,
/// which Go's decoders read apart.
const SURROGATE_PAIRING: &str = "json.surrogate-pairing";

/// Whether `doc` holds a number beyond float64's range (an exponent of three digits from 309).
fn has_out_of_range_number(doc: &[u8]) -> bool {
    doc.windows(4).any(|w| {
        matches!(w.first(), Some(b'e' | b'E'))
            && w.get(1..).is_some_and(|digits| {
                digits.iter().all(u8::is_ascii_digit) && digits >= b"309".as_slice()
            })
    })
}

/// Whether `doc` has a high surrogate escape followed by an escape that is not a low surrogate.
fn has_unpaired_escape(doc: &[u8]) -> bool {
    /// Whether the four hex digits of an escape are a surrogate's: `d`, then one of `second`.
    fn surrogate(digits: &[u8], second: &[u8]) -> bool {
        matches!(digits.first(), Some(b'd' | b'D'))
            && digits.get(1).is_some_and(|digit| second.contains(digit))
    }
    doc.windows(12).any(|w| {
        w.starts_with(br"\u")
            && w.get(2..6)
                .is_some_and(|digits| surrogate(digits, b"89abAB"))
            && w.get(6..8) == Some(br"\u".as_slice())
            && !w
                .get(8..12)
                .is_some_and(|digits| surrogate(digits, b"cdefCDEF"))
    })
}

/// The end of jsonparser's message for a value it cannot read as a number (Go:
/// jsonparser/parser.go, `MalformedValueError`), which `StringInt` fails with on `"+5"`.
const JSONPARSER_MALFORMED: &str =
    "Value looks like Number/Boolean/None, but can't find its end: ',' or '}' symbol";

/// `doc` with each number `-0` written `0`, `None` when it holds none or is not JSON: what the
/// port reads where Go reads `-0` into an integer as 0.
fn without_negative_zeros(doc: &str) -> Option<String> {
    fn fix(node: shapes::Node, changed: &mut bool) -> shapes::Node {
        match node {
            shapes::Node::Object(members) => shapes::Node::Object(
                members
                    .into_iter()
                    .map(|(key, value)| (key, fix(value, changed)))
                    .collect(),
            ),
            shapes::Node::Array(elements) => shapes::Node::Array(
                elements
                    .into_iter()
                    .map(|element| fix(element, changed))
                    .collect(),
            ),
            shapes::Node::Scalar(text) if text == "-0" => {
                *changed = true;
                shapes::Node::Scalar("0".to_owned())
            }
            scalar @ shapes::Node::Scalar(_) => scalar,
        }
    }
    let mut changed = false;
    let fixed = fix(shapes::Node::parse(doc)?, &mut changed);
    changed.then(|| {
        let mut out = String::new();
        fixed.write(&mut out);
        out
    })
}

/// Go's outcome of one decoder: its error, or the bytes it wrote (from the `GoJson` port where
/// Go wrote a hash the port's bytes match); `None` where Go's bytes are not known.
fn go_outcome(err: &[u8], want: &Want, gojson: &golden::Encoded) -> Option<Outcome> {
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    if !err.is_empty() {
        return Some(Outcome::DecodeFailed(text(err)));
    }
    match want {
        Want::Bytes(bytes) => Some(Outcome::Encoded(text(bytes))),
        Want::Error(e) => Some(Outcome::EncodeFailed(text(e))),
        Want::Hash { .. } => gojson
            .as_ref()
            .ok()
            .filter(|_| golden::matches(want, gojson))
            .map(|bytes| Outcome::Encoded(text(bytes))),
    }
}

/// The ledger entry of a document's difference that the comparison alone does not name, nor a
/// fix of the document explains: what the document holds that `serde_json` reads differently.
fn document_classes(std: bool, doc: &[u8], port: &Outcome, go: &Outcome) -> Option<&'static str> {
    if has_unpaired_escape(doc) {
        return Some(SURROGATE_PAIRING);
    }
    if let Outcome::DecodeFailed(g) = go {
        // jsoniter fails on such a number it skips; `encoding/json` reads them all.
        if !std
            && has_out_of_range_number(doc)
            && matches!(port, Outcome::Encoded(_) | Outcome::EncodeFailed(_))
            && (g.contains("invalid number") || g.contains("value out of range"))
        {
            return Some(OUT_OF_RANGE_NUMBERS);
        }
        if matches!(port, Outcome::Encoded(_))
            && g.ends_with(JSONPARSER_MALFORMED)
            && doc.windows(2).any(|w| w == b"\"+")
        {
            return Some(STRING_INT_PARSE);
        }
    }
    // `StringInt` fails on `"-"`, which jsonparser reads as 0.
    matches!(port, Outcome::DecodeFailed(m) if m.starts_with("invalid digit found in string"))
        .then_some(STRING_INT_PARSE)
}

/// Whether `json.Unmarshal` failed with the message of a Prebid `UnmarshalJSON` method where the
/// port failed on a value of the wrong type: `encoding/json` keeps a type error, decodes on and
/// returns a method's error over it.
fn is_saved_type_error(port: &Outcome, go: &Outcome) -> bool {
    matches!(
        (port, go),
        (Outcome::DecodeFailed(p), Outcome::DecodeFailed(g))
            if p.starts_with("invalid type: ") && shapes::is_prebid_message(g)
    )
}

/// How many fixes [`explain`] makes to a document, one on another: a key respelled as Go
/// matches it, or `-0` written `0`.
const FIXES: usize = 2;

/// The ledger entries the difference between the port's outcome of `doc` (`decode` decodes and
/// writes it) and Go's falls in, or what is unexplained. Where they differ, the document is
/// retried as the port reads it with `-0` written `0` (`json.negative-zero-int`), and with its
/// keys spelled as Go writes them, in this row or as the `GoJson` port writes them in any row of
/// the type (`type_keys`), up to `fixes` fixes deep.
fn explain(
    decode: shapes::Decode,
    std: bool,
    doc: &[u8],
    go: &Outcome,
    type_keys: &BTreeSet<String>,
    fixes: usize,
) -> Result<Vec<&'static str>, String> {
    let port = decode(doc)?;
    let unexplained = match shapes::compare(&String::from_utf8_lossy(doc), &port, go) {
        Ok(classes) => return Ok(classes),
        Err(e) => e,
    };
    if let Some(fixes) = fixes.checked_sub(1) {
        let failed = |start: &str| matches!(&port, Outcome::DecodeFailed(m) if m.contains(start));
        let Ok(text) = str::from_utf8(doc) else {
            // Go reads each byte that is not UTF-8 as U+FFFD.
            if failed("invalid unicode code point")
                && let Ok(mut classes) = explain(
                    decode,
                    std,
                    String::from_utf8_lossy(doc).as_bytes(),
                    go,
                    type_keys,
                    fixes,
                )
            {
                classes.push(INVALID_UTF8);
                return Ok(classes);
            }
            return Err(unexplained);
        };
        if failed("invalid type: floating point `-0.0`")
            && let Some(fixed) = without_negative_zeros(text)
            && let Ok(mut classes) = explain(decode, std, fixed.as_bytes(), go, type_keys, fixes)
        {
            classes.push(NEGATIVE_ZERO_INT);
            return Ok(classes);
        }
        let row_keys = shapes::go_keys(go);
        for keys in [&row_keys, type_keys] {
            if keys.is_empty() {
                continue;
            }
            for mode in shapes::RESPELLINGS {
                let Some(respelled) = shapes::respelled_keys(text, keys, mode) else {
                    continue;
                };
                let respelled_doc = respelled.doc.as_bytes();
                if let Ok(mut classes) = explain(decode, std, respelled_doc, go, type_keys, fixes) {
                    classes.extend(respelled.classes);
                    return Ok(classes);
                }
            }
        }
    }
    if std && is_saved_type_error(&port, go) {
        return Ok(vec![ERROR_TEXTS]);
    }
    document_classes(std, doc, &port, go)
        .map(|class| vec![class])
        .ok_or(unexplained)
}

/// The ledger entries one decoder's difference falls in, or what is unexplained.
fn classify(
    runner: &shapes::Runner,
    std: bool,
    doc: &[u8],
    go: &Outcome,
    type_keys: &BTreeSet<String>,
) -> Result<Vec<&'static str>, String> {
    let decode = if std {
        runner.decode_std
    } else {
        runner.decode
    };
    let mut found = explain(decode, std, doc, go, type_keys, FIXES)?;
    if std {
        found = shapes::std_classes(found);
    }
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// Keys of types that no encoding writes, by the Go type whose documents hold them: the members
/// of `ExtImpAppnexusKeyVal`, which the keywords are read from and written as one string.
const UNWRITTEN_KEYS: &[(&str, &[&str])] = &[
    ("openrtb_ext.ExtImpAppnexus", &["key", "value"]),
    ("openrtb_ext.ExtImpAppnexusKeywords", &["key", "value"]),
];

/// One decoder's outcome of a golden row, to classify.
struct Pending {
    /// The Go type.
    name: String,
    /// The row's label.
    label: String,
    /// The document.
    doc: Vec<u8>,
    /// Whether the decoder is `json.Unmarshal`, not `jsonutil.Unmarshal`.
    std: bool,
    /// Go's outcome.
    go: Outcome,
}

/// What the serde code did with a golden's rows.
#[derive(Default)]
struct Tally {
    /// The documents replayed.
    rows: usize,
    /// The ledger entries the differences fall in, with the number of outcomes of each.
    entries: BTreeMap<&'static str, usize>,
    /// The outcomes whose bytes Go wrote as a hash the `GoJson` port does not match.
    unverified: usize,
    /// The outcomes that differ outside the ledger.
    unexplained: Vec<String>,
}

/// [`UNWRITTEN_KEYS`], as the keys of each type that `replay` adds to.
fn unwritten_keys() -> BTreeMap<String, BTreeSet<String>> {
    UNWRITTEN_KEYS
        .iter()
        .map(|(name, keys)| {
            (
                (*name).to_owned(),
                keys.iter().map(|key| (*key).to_owned()).collect(),
            )
        })
        .collect()
}

/// Replays the golden `file` through the serde code.
fn replay(file: &str) -> TestResult<Tally> {
    let gojson = golden::runners()?;
    let serde = shapes::runners()?;
    let mut files = Vec::new();
    let mut tally = Tally::default();
    let mut pending = Vec::new();
    let mut type_keys = unwritten_keys();
    for row in golden_rows(file)? {
        match row.first().map(Vec::as_slice) {
            Some(b"#go" | b"#root") => continue,
            Some(b"#file") => {
                files.push(golden::corpus_file(&row)?.1);
                continue;
            }
            Some(_) | None => {}
        }
        let [
            doc_ref,
            go_type,
            label,
            iter_err,
            c1,
            c2,
            c3,
            std_err,
            c4,
            c5,
            c6,
        ] = row.as_slice()
        else {
            return Err(format!("{file}: bad row of {} fields", row.len()));
        };
        let doc = golden::document(doc_ref, &files)?;
        let name = String::from_utf8_lossy(go_type).into_owned();
        let Some(run_gojson) = gojson.get(&name) else {
            return Err(format!("{file}: no Rust type for Go type {name}"));
        };
        let ported = run_gojson(&doc);
        tally.rows += 1;
        // The `GoJson` port spells the keys as Go does, in what it read before a failure too.
        let keys = type_keys.entry(name.clone()).or_default();
        for bytes in ported.encoded.iter().flatten() {
            keys.extend(shapes::go_keys(&Outcome::Encoded(
                String::from_utf8_lossy(bytes).into_owned(),
            )));
        }
        let mut wants = Vec::new();
        for cell in [c1, c2, c3, c4, c5, c6] {
            let resolved = golden::want(cell, &doc, &wants)?;
            wants.push(resolved);
        }
        let (Some(first), Some(fourth)) = (wants.first(), wants.get(3)) else {
            return Err(format!("{file}: a row without six encodings"));
        };
        for (std, err, want, gojson_bytes) in [
            (false, iter_err, first, &ported.encoded[0]),
            (true, std_err, fourth, &ported.encoded[3]),
        ] {
            let Some(go) = go_outcome(err, want, gojson_bytes) else {
                tally.unverified += 1;
                continue;
            };
            pending.push(Pending {
                name: name.clone(),
                label: String::from_utf8_lossy(label).into_owned(),
                doc: doc.clone(),
                std,
                go,
            });
        }
    }
    let no_keys = BTreeSet::new();
    for case in pending {
        let Some(runner) = serde.get(&case.name) else {
            return Err(format!("{file}: no serde code for Go type {}", case.name));
        };
        let keys = type_keys.get(&case.name).unwrap_or(&no_keys);
        match classify(runner, case.std, &case.doc, &case.go, keys) {
            Ok(found) => {
                for class in found {
                    *tally.entries.entry(class).or_default() += 1;
                }
            }
            Err(e) => tally.unexplained.push(format!(
                "{} {} ({}): {e}",
                case.name,
                case.label,
                if case.std {
                    "json.Unmarshal"
                } else {
                    "jsonutil.Unmarshal"
                }
            )),
        }
    }
    Ok(tally)
}

/// Replays `file`: at least `min_rows` documents, every difference in the ledger entries of
/// `expected` (each of which some outcome shows), and Go's bytes known for every outcome.
fn check(file: &str, min_rows: usize, expected: &[&str]) -> TestResult {
    let tally = replay(file)?;
    report(&format!("{file} through serde"), &tally.unexplained, 200)?;
    if tally.rows < min_rows {
        return Err(format!("{file}: only {} documents", tally.rows));
    }
    ensure_eq(
        &tally.unverified,
        &0,
        &format!("{file}: outcomes without Go's bytes"),
    )?;
    ensure_eq(
        &tally.entries.keys().copied().collect::<BTreeSet<_>>(),
        &expected.iter().copied().collect(),
        &format!(
            "{file}: the ledger entries the serde code's differences fall in ({:?})",
            tally.entries
        ),
    )
}

#[test]
fn corpus_differs_only_in_ledger_entries() -> TestResult {
    check(
        "corpus.tsv",
        4000,
        &[
            DUPLICATE_KEYS,
            ERROR_TEXTS,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            KEY_CASE,
            NIL_RAW_WRITTEN,
            NUMBER_TEXT,
            PREBID_MESSAGE,
            STD_ERROR_TEXTS,
            STRUCT_FROM_ARRAY,
        ],
    )
}

#[test]
fn mutations_differ_only_in_ledger_entries() -> TestResult {
    check(
        "mutations.tsv",
        19000,
        &[
            DUPLICATE_KEYS,
            ERROR_TEXTS,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            INVALID_UTF8,
            KEY_CASE,
            NEGATIVE_ZERO_INT,
            NIL_RAW_WRITTEN,
            NULL_DOCUMENT,
            NULL_ELEMENTS,
            NUMBER_TEXT,
            PREBID_MESSAGE,
            STD_ERROR_TEXTS,
            STRING_OR_ARRAY_NULL,
            STRUCT_FROM_ARRAY,
            SURROGATE_PAIRING,
        ],
    )
}

#[test]
fn review_cases_differ_only_in_ledger_entries() -> TestResult {
    check(
        "review.tsv",
        8000,
        &[
            DUPLICATE_KEYS,
            ERROR_TEXTS,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            KEY_CASE,
            NEGATIVE_ZERO_INT,
            NIL_RAW_WRITTEN,
            NULL_DOCUMENT,
            NULL_ELEMENTS,
            NUMBER_TEXT,
            OUT_OF_RANGE_NUMBERS,
            PREBID_MESSAGE,
            STD_ERROR_TEXTS,
            STRING_INT_PARSE,
            STRING_OR_ARRAY_NULL,
            STRUCT_FROM_ARRAY,
        ],
    )
}

/// The review's merges (`review_merges.jsonl`, written by `testdata/go/review_merges.go`): the
/// values the types with Go JSON methods tell apart merged over values that already hold
/// something, through the pointer, value and map fields that hold them, and the `openrtb_ext`
/// types an account holds merged with fields in their map entries. Each merge compares as the
/// shape cases' merges do, and every difference falls in a ledger entry.
#[test]
fn review_merges_differ_only_in_ledger_entries() -> TestResult {
    let cases = shapes::cases_in("review_merges.jsonl")?;
    if cases.len() < 2600 {
        return Err(format!("review_merges.jsonl: only {} cases", cases.len()));
    }
    let (classes, unexplained) = shapes::run(&shapes::runners()?, &cases);
    shapes::ensure_explained(&unexplained)?;
    ensure_eq(
        &classes.keys().copied().collect::<BTreeSet<_>>(),
        &[
            DUPLICATE_KEYS,
            ERROR_TEXTS,
            FAILED_MERGE_ELEMENTS,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            KEY_CASE,
            NIL_RAW_WRITTEN,
            NULL_ELEMENTS,
            PARTIAL_RESULTS,
            PREBID_MESSAGE,
            STRUCT_FROM_ARRAY,
        ]
        .into_iter()
        .collect(),
        &format!("review_merges.jsonl: the ledger entries the differences fall in ({classes:?})"),
    )
}
