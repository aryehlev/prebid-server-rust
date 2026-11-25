//! The serde code against the goldens that `golden` replays through `GoJson`
//! (`testdata/go/main.go` documents them).
//!
//! Each row's document is decoded with `pbs_jsonutil::unmarshal`, and through `EncodingJson`,
//! and the value written with `marshal`, against Go's `jsonutil.Unmarshal` and `json.Unmarshal`
//! values written with `jsonutil.Marshal` (Go: util/jsonutil/jsonutil.go:136-144 and 159-167; the
//! row's first and fourth encodings). Outcomes compare as `shapes` compares them, and a difference
//! beyond `shapes`' classes must fall in a ledger entry, which [`entry`] names from what each side
//! did and, where it can, from the port's outcome once the document says what Go read. The
//! documents of prebid-server's own corpora differ in no other way than the key case of the
//! fixtures [`KEY_CASE_FIXTURES`] lists.

use core::str::from_utf8;
use std::collections::{BTreeMap, BTreeSet};

use pbs_gojson::gotype::go_type_name;
use pbs_jsonutil::{marshal, unmarshal};
use pbs_openrtb::{adcom1, gojson_bridge::EncodingJson, native1, openrtb2, openrtb3};
use serde::{Serialize, de::DeserializeOwned};

use super::{
    golden::{self, Want},
    index::for_each_type,
    shapes::{
        self, ERROR_TEXTS, FLOAT_FORMAT, HTML_ESCAPE, NIL_RAW_WRITTEN, NULL_DOCUMENT, Outcome,
        STD_ERROR_TEXTS,
    },
    support::{TestResult, ensure_eq, golden_rows, report, show},
};

/// `json.duplicate-keys`: the derives reject a repeated key.
const DUPLICATE_KEYS: &str = "json.duplicate-keys";
/// `json.invalid-utf8`: `serde_json` rejects invalid UTF-8 in a string.
const INVALID_UTF8: &str = "json.invalid-utf8";
/// `json.key-case`: a key spelled in another case is unknown to serde.
const KEY_CASE: &str = "json.key-case";
/// `json.lone-surrogates-raw`: a lone surrogate escape in a raw member is written as U+FFFD's.
const LONE_SURROGATES_RAW: &str = "json.lone-surrogates-raw";
/// `json.negative-zero-int`: `serde_json` reads `-0` as a float, which no integer field takes.
const NEGATIVE_ZERO_INT: &str = "json.negative-zero-int";
/// `json.nesting-depth`: `serde_json` reads a raw member, or skips an unknown one, at any depth.
const NESTING_DEPTH: &str = "json.nesting-depth";
/// `json.null-elements`: a null slice element fails the document, where Go reads a zero value.
const NULL_ELEMENTS: &str = "json.null-elements";
/// `json.out-of-range-numbers`: `serde_json` skips a number beyond float64's range.
const OUT_OF_RANGE_NUMBERS: &str = "json.out-of-range-numbers";
/// `json.struct-from-array`: a derived struct reads an array as its fields.
const STRUCT_FROM_ARRAY: &str = "json.struct-from-array";

/// A key as a document spells it, and as its field names it.
type Respelling = (&'static str, &'static str);

/// The fixtures of prebid-server's corpora that spell a field's key in another case, by
/// directory: each key as spelled and as the field names it, and how many of the directory's
/// documents differ, which, so respelled, decode as Go decodes them. Beachfront's bid responses
/// spell `seatbid` `seatBid`, so a serde decode reads no bids from them, where Go's beachfront
/// adapter reads them (adapters/beachfront/beachfront.go:646).
const KEY_CASE_FIXTURES: &[(&str, &[Respelling], usize)] = &[
    ("adapters/adgeneration/", &[("TID", "tid")], 1),
    ("adapters/beachfront/", &[("seatBid", "seatbid")], 8),
    (
        "adapters/invibes/",
        &[("buyerUid", "buyeruid"), ("IP", "ip"), ("IPv6", "ipv6")],
        8,
    ),
    (
        "adapters/tadvertising_blis/",
        &[("deviceType", "devicetype")],
        7,
    ),
    (
        "adapters/yandex/",
        &[
            ("DisplayManager", "displaymanager"),
            ("DisplayManagerVer", "displaymanagerver"),
        ],
        1,
    ),
];

/// The endings of the generator's labels for documents that spell a key in another case and
/// differ in nothing else (testdata/go/mutations.go:85-90, adversarial.go:175-201).
const KEY_CASE_LABELS: &[&str] = &[
    " upper-case key",
    " title-case key",
    " key then upper-case key",
    " upper-case key then key",
];

/// The row's first decoder, which prebid-server decodes with.
const JSONUTIL: &str = "jsonutil.Unmarshal";
/// The row's second decoder, which `EncodingJson` stands for.
const ENCODING_JSON: &str = "json.Unmarshal";
/// The decoders of a row, in its order.
const DECODERS: [&str; 2] = [JSONUTIL, ENCODING_JSON];

/// `value` written with `marshal`.
fn written<T: Serialize>(value: &T) -> Outcome {
    match marshal(value) {
        Ok(bytes) => Outcome::Encoded(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) => Outcome::EncodeFailed(e.to_string()),
    }
}

/// `doc` through the serde code of `T`, decoded with `unmarshal` (through `EncodingJson` when
/// `encoding_json` is set) and written back.
fn serde_outcome<T: DeserializeOwned + Serialize>(doc: &[u8], encoding_json: bool) -> Outcome {
    let decoded = if encoding_json {
        EncodingJson::<T>::unmarshal(doc)
    } else {
        unmarshal::<T>(doc)
    };
    decoded.map_or_else(
        |e| Outcome::DecodeFailed(e.to_string()),
        |value| written(&value),
    )
}

/// Runs one document through the serde code of a ported type, as `json.Unmarshal` decodes when
/// the flag is set.
type SerdeRunner = fn(&[u8], bool) -> Outcome;

/// The serde runner of each ported type, by Go name.
fn serde_runners() -> BTreeMap<String, SerdeRunner> {
    let mut by_name: BTreeMap<String, SerdeRunner> = BTreeMap::new();
    macro_rules! runner {
        ($ty:ty) => {{
            let _ = by_name.insert(go_type_name::<$ty>(), serde_outcome::<$ty>);
        }};
    }
    for_each_type!(runner);
    by_name
}

/// Go's outcome of one decoder from its row: its error text, or the value's `jsonutil.Marshal`
/// bytes; `None` where the decode succeeded and the golden holds only a hash of bytes `GoJson`
/// does not write.
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

/// The byte offset in `doc` just past the token that `serde_json`'s error `message` points at.
fn error_offset(doc: &[u8], message: &str) -> Option<usize> {
    let (_, at) = message.rsplit_once(" at line ")?;
    let (line, column) = at.split_once(" column ")?;
    let line = line.parse::<usize>().ok()?.checked_sub(1)?;
    let start: usize = doc
        .split_inclusive(|&b| b == b'\n')
        .take(line)
        .map(<[u8]>::len)
        .sum();
    start.checked_add(column.parse().ok()?)
}

/// `doc` with the token `token` that `serde_json`'s error `message` points at spelled `with`.
fn splice(doc: &[u8], message: &str, token: &[u8], with: &str) -> Option<Vec<u8>> {
    let end = error_offset(doc, message)?;
    let before = doc.get(..end)?.strip_suffix(token)?;
    Some([before, with.as_bytes(), doc.get(end..)?].concat())
}

/// The JSON of the zero value of what `serde_json` expected (`a string`, `i64`, `struct Format`).
fn zero_value(expected: &str) -> Option<&'static str> {
    if expected.starts_with("struct ") {
        return Some("{}");
    }
    Some(match expected {
        "a string" => "\"\"",
        "a boolean" => "false",
        "a sequence" => "[]",
        "a map" => "{}",
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" | "f32"
        | "f64" => "0",
        _ => return None,
    })
}

/// `doc` with each byte that starts no UTF-8 sequence read as U+FFFD, as Go's decoders read it.
fn as_go_utf8(doc: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(doc.len());
    let mut rest = doc;
    loop {
        match from_utf8(rest) {
            Ok(valid) => {
                out.extend_from_slice(valid.as_bytes());
                return out;
            }
            Err(e) => {
                let (valid, invalid) = rest.split_at(e.valid_up_to());
                out.extend_from_slice(valid);
                out.extend_from_slice("\u{fffd}".as_bytes());
                rest = invalid.get(1..).unwrap_or_default();
            }
        }
    }
}

/// `doc` as Go reads what `serde_json`'s error `message` rejects, and the ledger entry of that
/// reading: a null slice element or map value as the zero value the message names
/// (`json.null-elements`; `shapes` pins that every field reads null), `-0` as `0`
/// (`json.negative-zero-int`), and each byte that starts no UTF-8 sequence as U+FFFD
/// (`json.invalid-utf8`).
fn repair(doc: &[u8], message: &str) -> Option<(Vec<u8>, &'static str)> {
    if let Some(expected) = message.strip_prefix("invalid type: null, expected ") {
        let zero = zero_value(expected.split(" at line ").next()?)?;
        return Some((splice(doc, message, b"null", zero)?, NULL_ELEMENTS));
    }
    if message.starts_with("invalid type: floating point `-0.0`, expected ") {
        return Some((splice(doc, message, b"-0", "0")?, NEGATIVE_ZERO_INT));
    }
    if message.starts_with("invalid unicode code point") && from_utf8(doc).is_err() {
        return Some((as_go_utf8(doc), INVALID_UTF8));
    }
    None
}

/// The outcome of `doc` decoded again after each [`repair`] the port's errors ask for, the first
/// error being `first`, and the repairs' ledger entries; `None` when nothing repairs `first`.
fn repaired(
    doc: &[u8],
    first: &str,
    rerun: &dyn Fn(&[u8]) -> Outcome,
) -> Option<(Outcome, Vec<&'static str>)> {
    let (mut doc, class) = repair(doc, first)?;
    let mut entries = vec![class];
    loop {
        let outcome = rerun(&doc);
        let again = match &outcome {
            Outcome::DecodeFailed(error) => repair(&doc, error),
            Outcome::Encoded(_) | Outcome::EncodeFailed(_) => None,
        };
        let Some((fixed, found)) = again else {
            return Some((outcome, entries));
        };
        if !entries.contains(&found) {
            entries.push(found);
        }
        doc = fixed;
    }
}

/// Whether the port's outcome is the derives' rejection of a key that `doc` repeats
/// (`json.duplicate-keys`).
fn rejects_repeated_key(port: &Outcome, doc: &[u8]) -> bool {
    let Outcome::DecodeFailed(message) = port else {
        return false;
    };
    message
        .strip_prefix("duplicate field `")
        .and_then(|rest| rest.split_once('`'))
        .is_some_and(|(key, _)| {
            let quoted = format!("\"{key}\"");
            String::from_utf8_lossy(doc)
                .matches(&quoted)
                .nth(1)
                .is_some()
        })
}

/// The JSON text `text` with each lone surrogate escape (`\ud800`) spelled as U+FFFD's.
fn without_lone_surrogates(text: &str) -> String {
    let unit = |escape: Option<&str>| {
        let hex = escape?.strip_prefix("\\u")?.get(..4)?;
        u16::from_str_radix(hex, 16).ok()
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('\\') {
        let (head, escape) = rest.split_at(at);
        out.push_str(head);
        let (spelled, len) = match (unit(Some(escape)), unit(escape.get(6..))) {
            (Some(0xD800..=0xDBFF), Some(0xDC00..=0xDFFF)) => (None, 12),
            (Some(0xD800..=0xDFFF), _) => (Some("\\uFFFD"), 6),
            (Some(_), _) => (None, 6),
            (None, _) => (None, 2),
        };
        out.push_str(spelled.or_else(|| escape.get(..len)).unwrap_or(escape));
        rest = escape.get(len..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// One golden row as [`entry`] reads it.
struct Row<'a> {
    /// The corpus file the document was cut from, empty for a generated document.
    source: &'a str,
    /// The generator's label.
    label: &'a str,
    /// The document.
    doc: &'a [u8],
}

/// The ledger entries that a difference beyond `shapes`' classes falls in, from what each side
/// did, what the document shows and, where Go read what the port rejects, the port's outcome once
/// the document says what Go read (`rerun` decodes a document as `port` was decoded); `None` when
/// no entry explains it.
fn entry(
    row: &Row<'_>,
    port: &Outcome,
    go: &Outcome,
    rerun: &dyn Fn(&[u8]) -> Outcome,
) -> Option<Vec<&'static str>> {
    let doc = String::from_utf8_lossy(row.doc);
    match (port, go) {
        (Outcome::DecodeFailed(message), Outcome::Encoded(_)) => {
            if rejects_repeated_key(port, row.doc) {
                return Some(vec![DUPLICATE_KEYS]);
            }
            let (outcome, mut entries) = repaired(row.doc, message, rerun)?;
            entries.extend(shapes::compare(&doc, &outcome, go).ok()?);
            Some(entries)
        }
        (Outcome::Encoded(_), Outcome::DecodeFailed(message)) => {
            if message.ends_with("but found [")
                || message.contains("cannot unmarshal array into Go")
            {
                Some(vec![STRUCT_FROM_ARRAY])
            } else if message.ends_with("exceeded max depth") {
                Some(vec![NESTING_DEPTH])
            } else if message == "invalid number" || message == "value out of range" {
                Some(vec![OUT_OF_RANGE_NUMBERS])
            } else {
                None
            }
        }
        (Outcome::Encoded(_), Outcome::Encoded(written)) => {
            let fixture = KEY_CASE_FIXTURES
                .iter()
                .find(|(dir, ..)| row.source.starts_with(dir));
            if let Some(&(_, keys, _)) = fixture {
                let mut respelled = doc.into_owned();
                for (spelled, key) in keys {
                    respelled =
                        respelled.replace(&format!("\"{spelled}\":"), &format!("\"{key}\":"));
                }
                let outcome = rerun(respelled.as_bytes());
                let mut entries = vec![KEY_CASE];
                entries.extend(shapes::compare(&respelled, &outcome, go).ok()?);
                return Some(entries);
            }
            if KEY_CASE_LABELS.iter().any(|end| row.label.ends_with(end))
                || row.label.contains(" fold U+")
            {
                return Some(vec![KEY_CASE]);
            }
            let unpaired = Outcome::Encoded(without_lone_surrogates(written));
            if unpaired == *go {
                return None;
            }
            let mut entries = vec![LONE_SURROGATES_RAW];
            entries.extend(shapes::compare(&doc, port, &unpaired).ok()?);
            Some(entries)
        }
        _ => None,
    }
}

/// What the replay of one golden file found.
#[derive(Default)]
struct Tally {
    /// Documents replayed.
    rows: usize,
    /// The decodes each ledger entry explains.
    entries: BTreeMap<&'static str, usize>,
    /// The key case documents of each [`KEY_CASE_FIXTURES`] directory, by decoder.
    fixtures: BTreeMap<(&'static str, &'static str), usize>,
    /// The labels of the decodes whose Go bytes the golden holds only a hash of, which no entry
    /// explains without them.
    unverified: BTreeMap<String, usize>,
    /// What no entry explains.
    unexplained: Vec<String>,
}

impl Tally {
    /// Counts one decode of `row` by `decoder`, which `rerun` repeats on another document.
    fn add(
        &mut self,
        decoder: &'static str,
        row: &Row<'_>,
        (port, go): (&Outcome, Option<&Outcome>),
        rerun: &dyn Fn(&[u8]) -> Outcome,
    ) {
        let Some(go) = go else {
            // Go decoded the document, and the golden holds a hash of bytes `GoJson` does not
            // write (`golden`'s stale elements): a rejected repeated key needs no bytes.
            if rejects_repeated_key(port, row.doc) {
                *self.entries.entry(DUPLICATE_KEYS).or_default() += 1;
            } else {
                *self.unverified.entry(row.label.to_owned()).or_default() += 1;
            }
            return;
        };
        let doc = String::from_utf8_lossy(row.doc);
        let found = match shapes::compare(&doc, port, go) {
            Ok(classes) => classes,
            Err(why) => {
                if let Some(classes) = entry(row, port, go, rerun) {
                    classes
                } else {
                    self.unexplained.push(format!(
                        "{decoder} {} {}: {why}; document {}",
                        row.source,
                        row.label,
                        show(row.doc)
                    ));
                    return;
                }
            }
        };
        let classes = if decoder == ENCODING_JSON {
            shapes::std_classes(found)
        } else {
            found
        };
        for class in classes {
            *self.entries.entry(class).or_default() += 1;
            if class == KEY_CASE {
                for &(dir, ..) in KEY_CASE_FIXTURES {
                    if row.source.starts_with(dir) {
                        *self.fixtures.entry((dir, decoder)).or_default() += 1;
                    }
                }
            }
        }
    }
}

/// Replays `file` through the serde code.
fn replay(file: &str) -> TestResult<Tally> {
    let gojson = golden::runners()?;
    let serde = serde_runners();
    let mut paths: Vec<String> = Vec::new();
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut tally = Tally::default();
    for row in golden_rows(file)? {
        match row.first().map(Vec::as_slice) {
            Some(b"#go" | b"#root") => continue,
            Some(b"#file") => {
                let (path, data) = golden::corpus_file(&row)?;
                paths.push(path);
                files.push(data);
                continue;
            }
            Some(_) | None => {}
        }
        let [
            doc_ref,
            ty,
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
            return Err(format!("{file}: a row of {} fields", row.len()));
        };
        let doc = golden::document(doc_ref, &files)?;
        let source = doc_ref
            .strip_prefix(b"@")
            .and_then(|r| r.split(|&b| b == b':').next())
            .and_then(|i| from_utf8(i).ok()?.parse::<usize>().ok())
            .and_then(|i| paths.get(i))
            .map_or("", String::as_str);
        let name = String::from_utf8_lossy(ty).into_owned();
        let gojson_run = gojson
            .get(&name)
            .ok_or_else(|| format!("no GoJson runner for {name}"))?;
        let serde_run = serde
            .get(&name)
            .ok_or_else(|| format!("no serde runner for {name}"))?;
        let mut wants: Vec<Want> = Vec::new();
        for cell in [c1, c2, c3, c4, c5, c6] {
            let resolved = golden::want(cell, &doc, &wants)?;
            wants.push(resolved);
        }
        let gojson_out = gojson_run(&doc);
        let label = String::from_utf8_lossy(label);
        let current = Row {
            source,
            label: &label,
            doc: &doc,
        };
        for (decoder, err, at, encoding_json) in [
            (JSONUTIL, iter_err, 0, false),
            (ENCODING_JSON, std_err, 3, true),
        ] {
            let (Some(want), Some(encoded)) = (wants.get(at), gojson_out.encoded.get(at)) else {
                return Err(format!("{file}: a short row"));
            };
            let port = serde_run(&doc, encoding_json);
            let go = go_outcome(err, want, encoded);
            let rerun = |other: &[u8]| serde_run(other, encoding_json);
            tally.add(decoder, &current, (&port, go.as_ref()), &rerun);
        }
        tally.rows += 1;
    }
    Ok(tally)
}

/// Replays `file`, then checks that every difference fell in a ledger entry, the entries being
/// exactly `expected`, and that at least `min_rows` documents were replayed.
fn check(file: &str, min_rows: usize, expected: &[&str]) -> TestResult<Tally> {
    let tally = replay(file)?;
    report(&format!("{file} through serde"), &tally.unexplained, 20)?;
    if tally.rows < min_rows {
        return Err(format!("{file}: only {} documents", tally.rows));
    }
    let seen: BTreeSet<&str> = tally.entries.keys().copied().collect();
    let want: BTreeSet<&str> = expected.iter().copied().collect();
    ensure_eq(
        &seen,
        &want,
        &format!("{file}: the ledger entries the serde code's differences fall in"),
    )?;
    Ok(tally)
}

#[test]
fn corpus_differs_only_lexically_and_in_key_case() -> TestResult {
    let tally = check(
        "corpus.tsv",
        8100,
        &[
            ERROR_TEXTS,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            KEY_CASE,
            STD_ERROR_TEXTS,
        ],
    )?;
    let want: BTreeMap<(&str, &str), usize> = KEY_CASE_FIXTURES
        .iter()
        .flat_map(|&(dir, _, n)| DECODERS.map(|decoder| ((dir, decoder), n)))
        .collect();
    ensure_eq(
        &tally.fixtures,
        &want,
        "corpus.tsv: the key case documents of each fixture directory",
    )?;
    ensure_eq(
        &tally.unverified,
        &BTreeMap::new(),
        "corpus.tsv: decodes without Go's bytes",
    )
}

#[test]
fn module_differs_only_lexically() -> TestResult {
    let tally = check("module.tsv", 30, &[FLOAT_FORMAT, HTML_ESCAPE])?;
    ensure_eq(
        &tally.unverified,
        &BTreeMap::new(),
        "module.tsv: decodes without Go's bytes",
    )
}

#[test]
fn mutations_differ_in_ledger_entries() -> TestResult {
    let tally = check(
        "mutations.tsv",
        20000,
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
            STD_ERROR_TEXTS,
            STRUCT_FROM_ARRAY,
        ],
    )?;
    ensure_eq(
        &tally.unverified,
        &BTreeMap::new(),
        "mutations.tsv: decodes without Go's bytes",
    )
}

#[test]
fn adversarial_documents_differ_in_ledger_entries() -> TestResult {
    let tally = check(
        "adversarial.tsv",
        6000,
        &[
            DUPLICATE_KEYS,
            ERROR_TEXTS,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            INVALID_UTF8,
            KEY_CASE,
            LONE_SURROGATES_RAW,
            NEGATIVE_ZERO_INT,
            NESTING_DEPTH,
            NIL_RAW_WRITTEN,
            NULL_ELEMENTS,
            OUT_OF_RANGE_NUMBERS,
            STD_ERROR_TEXTS,
            STRUCT_FROM_ARRAY,
        ],
    )?;
    ensure_eq(
        &tally.unverified,
        &BTreeMap::new(),
        "adversarial.tsv: decodes without Go's bytes",
    )
}
