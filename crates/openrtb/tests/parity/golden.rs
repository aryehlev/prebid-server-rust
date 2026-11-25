//! Replays the goldens that `testdata/go` writes (its `main.go` documents the format): every
//! decoder error and every encoding must match Go byte for byte.

use core::{iter::repeat_n, str::from_utf8};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, thread,
};

use pbs_gojson::{GoEncode, JsoniterDecode, StdDecode, gotype::go_type_name, jsonutil, stdjson};
use pbs_openrtb::{adcom1, native1, openrtb2, openrtb3};
use serde_json::Value;

use super::{
    index::{TYPES, for_each_type},
    support::{
        TestResult, ensure_eq, fnv64, go_root, golden_rows, read, report, show, workspace_root,
    },
};

/// One encoding: its bytes, or its error text.
pub(super) type Encoded = Result<Vec<u8>, Vec<u8>>;

/// What Rust does with one document, in the order of a golden row: the error text of each
/// decoder (empty on success), and the value each decoder leaves (also after an error)
/// encoded with `jsonutil.Marshal`, `json.Marshal` and an `Encoder` without HTML escaping.
pub(super) struct Outcome {
    /// `jsonutil.Unmarshal`'s error text.
    pub(super) jsoniter_err: Vec<u8>,
    /// `json.Unmarshal`'s error text.
    pub(super) std_err: Vec<u8>,
    /// The `jsonutil.Unmarshal` value's encodings, then the `json.Unmarshal` value's.
    pub(super) encoded: [Encoded; 6],
}

/// Runs one document through the Rust port of a Go type.
pub(super) type Runner = fn(&[u8]) -> Outcome;

/// The runner of each ported type, by Go name.
pub(super) type Runners = BTreeMap<String, Runner>;

/// `v` encoded with `jsonutil.Marshal`, `json.Marshal` and an `Encoder` without HTML escaping.
pub(super) fn encode<T: GoEncode>(v: &T) -> [Encoded; 3] {
    [
        jsonutil::marshal(v).map_err(|e| e.message.into_bytes()),
        stdjson::marshal(v).map_err(|e| e.go_text()),
        stdjson::encode_no_escape_html(v).map_err(|e| e.go_text()),
    ]
}

/// What the generator does with a document: decode it into a new zero value with each
/// decoder, then encode each value three ways.
fn outcome<T: Default + GoEncode + JsoniterDecode + StdDecode>(doc: &[u8]) -> Outcome {
    let mut jsoniter_value = T::default();
    let jsoniter_err = match jsonutil::unmarshal(doc, &mut jsoniter_value) {
        Ok(()) => Vec::new(),
        Err(e) => e.message.into_bytes(),
    };
    let mut std_value = T::default();
    let std_err = match stdjson::unmarshal(doc, &mut std_value) {
        Ok(()) => Vec::new(),
        Err(e) => e.go_text(),
    };
    let [e1, e2, e3] = encode(&jsoniter_value);
    let [e4, e5, e6] = encode(&std_value);
    Outcome {
        jsoniter_err,
        std_err,
        encoded: [e1, e2, e3, e4, e5, e6],
    }
}

/// The runner of every ported type by Go name; fails unless the Go names are those of
/// [`TYPES`], in order, and each names one Rust type.
pub(super) fn runners() -> TestResult<Runners> {
    let mut by_name = Runners::new();
    let mut names = Vec::new();
    macro_rules! runner {
        ($ty:ty) => {{
            let name = go_type_name::<$ty>();
            names.push(name.clone());
            if by_name.insert(name, outcome::<$ty>).is_some() {
                return Err(format!(
                    "two Rust types for Go type {}",
                    go_type_name::<$ty>()
                ));
            }
        }};
    }
    for_each_type!(runner);
    let want: Vec<String> = TYPES.iter().map(|&name| name.to_owned()).collect();
    ensure_eq(&names, &want, "the Go names of the ported types")?;
    Ok(by_name)
}

/// A golden encoding, resolved to what Rust must produce.
#[derive(Clone)]
pub(super) enum Want {
    /// These bytes.
    Bytes(Vec<u8>),
    /// Bytes of this length and FNV-1a 64 hash, then a newline when `newline` is set.
    Hash { len: usize, fnv: u64, newline: bool },
    /// This error text.
    Error(Vec<u8>),
}

/// Resolves an encoding cell of a row whose document is `doc`; `earlier` are the row's
/// encodings before it.
pub(super) fn want(cell: &[u8], doc: &[u8], earlier: &[Want]) -> TestResult<Want> {
    let bad = || format!("bad encoding cell {}", show(cell));
    let (&tag, rest) = cell.split_first().ok_or_else(bad)?;
    match tag {
        b'~' => Ok(Want::Bytes(doc.to_vec())),
        b'\'' => Ok(Want::Bytes(rest.to_vec())),
        b'!' => Ok(Want::Error(rest.to_vec())),
        b'#' => {
            let text = from_utf8(rest).map_err(|e| format!("{}: {e}", bad()))?;
            let (len, fnv) = text.split_once(':').ok_or_else(bad)?;
            Ok(Want::Hash {
                len: len.parse().map_err(|e| format!("{}: {e}", bad()))?,
                fnv: u64::from_str_radix(fnv, 16).map_err(|e| format!("{}: {e}", bad()))?,
                newline: false,
            })
        }
        b'=' | b'+' => {
            let n: usize = from_utf8(rest)
                .ok()
                .and_then(|n| n.parse().ok())
                .ok_or_else(bad)?;
            let base = earlier.get(n.wrapping_sub(1)).ok_or_else(bad)?.clone();
            if tag == b'=' {
                return Ok(base);
            }
            match base {
                Want::Bytes(mut bytes) => {
                    bytes.push(b'\n');
                    Ok(Want::Bytes(bytes))
                }
                Want::Hash {
                    len,
                    fnv,
                    newline: false,
                } => Ok(Want::Hash {
                    len,
                    fnv,
                    newline: true,
                }),
                Want::Hash { newline: true, .. } | Want::Error(_) => Err(bad()),
            }
        }
        _ => Err(bad()),
    }
}

pub(super) fn matches(want: &Want, got: &Encoded) -> bool {
    match (want, got) {
        (Want::Bytes(want), Ok(got)) | (Want::Error(want), Err(got)) => want == got,
        (Want::Hash { len, fnv, newline }, Ok(got)) => {
            let body = if *newline {
                got.strip_suffix(b"\n")
            } else {
                Some(got.as_slice())
            };
            body.is_some_and(|body| body.len() == *len && fnv64(body) == *fnv)
        }
        (Want::Bytes(_) | Want::Hash { .. }, Err(_)) | (Want::Error(_), Ok(_)) => false,
    }
}

fn describe_want(want: &Want) -> String {
    match want {
        Want::Bytes(bytes) => show(bytes),
        Want::Hash { len, fnv, newline } => {
            format!("{len} bytes hashing to {fnv:016x} (then a newline: {newline})")
        }
        Want::Error(text) => format!("error {}", show(text)),
    }
}

fn describe_got(got: &Encoded) -> String {
    match got {
        Ok(bytes) => show(bytes),
        Err(text) => format!("error {}", show(text)),
    }
}

/// The six bytes Go writes for each invalid UTF-8 byte of a string: U+FFFD as a JSON escape.
const GO_INVALID_BYTE: &[u8] = concat!(r"\u", "fffd").as_bytes();

/// U+FFFD in UTF-8: what a Rust `String` holds where the Go string held an invalid byte.
const REPLACEMENT: [u8; 3] = [0xEF, 0xBF, 0xBD];

/// `bytes` with each U+FFFD written as Go writes an invalid byte.
fn as_go_invalid_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        if let Some(tail) = rest.strip_prefix(REPLACEMENT.as_slice()) {
            out.extend_from_slice(GO_INVALID_BYTE);
            rest = tail;
        } else if let Some((&b, tail)) = rest.split_first() {
            out.push(b);
            rest = tail;
        } else {
            return out;
        }
    }
}

/// Counts from replaying one golden file.
#[derive(Default)]
struct Tally {
    /// Documents replayed.
    rows: usize,
    /// Outputs compared (two error texts and six encodings per document).
    outputs: usize,
    /// Encodings that differ only as the documented `String` gap allows.
    string_gaps: usize,
    /// Rows of [`STALE_ELEMENT`] cases that differ from Go (the documented `Vec` gap).
    stale_elements: usize,
    /// Every other difference.
    failures: Vec<String>,
    /// The directories of the `#root` rows.
    roots: Vec<String>,
    /// The paths of the `#file` rows.
    files: Vec<String>,
}

/// The end of the labels of the adversarial cases that revive slice elements beyond a slice's
/// length.
///
/// Go decodes an array into the slice it already holds: it reuses the elements up to the
/// slice's capacity, also those beyond its length, which a shorter array decoded earlier left
/// in the backing array. A third occurrence of a key (`{"imp":[A,B],"imp":[C],"imp":[{},{}]}`)
/// therefore brings back B, merged with what the third array says about it, while a Rust `Vec`
/// dropped B when it shrank (Go: `encoding/json` `decodeState.array`, jsoniter
/// `sliceDecoder.doDecode` through reflect2's `UnsafeGrow`). Besides such repeated keys, only Go
/// code that decodes into a slice it cut shorter (`s = s[:n]`) can show it.
const STALE_ELEMENT: &[u8] = b"stale element";

/// The message for a corpus that changed after the goldens were written.
fn regenerate(what: &str) -> String {
    format!(
        "{what} after the goldens were written; regenerate them with \
         `GOTOOLCHAIN=go1.26.2 go run .` in crates/openrtb/testdata/go"
    )
}

/// A corpus file of a `#file` row: its path and bytes, checked against the hash Go recorded.
pub(super) fn corpus_file(row: &[Vec<u8>]) -> TestResult<(String, Vec<u8>)> {
    let [_, path, hash] = row else {
        return Err(format!("bad #file row of {} fields", row.len()));
    };
    let path = String::from_utf8_lossy(path).into_owned();
    let data = read(&go_root()?.join(&path))?;
    let got = format!("{:016x}", fnv64(&data));
    if got.as_bytes() != hash.as_slice() {
        return Err(regenerate(&format!("{path} changed")));
    }
    Ok((path, data))
}

/// The document of a row: `'` and its bytes, or `@file:offset:length` into a corpus file.
pub(super) fn document(field: &[u8], files: &[Vec<u8>]) -> TestResult<Vec<u8>> {
    if let Some(inline) = field.strip_prefix(b"'") {
        return Ok(inline.to_vec());
    }
    let bad = || format!("bad document reference {}", show(field));
    let reference = field
        .strip_prefix(b"@")
        .and_then(|r| from_utf8(r).ok())
        .ok_or_else(bad)?;
    let parts: Vec<usize> = reference
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", bad()))?;
    let [file, offset, len] = parts.as_slice() else {
        return Err(bad());
    };
    let end = offset.checked_add(*len).ok_or_else(bad)?;
    files
        .get(*file)
        .and_then(|data| data.get(*offset..end))
        .map(<[u8]>::to_vec)
        .ok_or_else(bad)
}

/// Replays one row of a golden file.
fn replay_row(
    row: &[Vec<u8>],
    files: &[Vec<u8>],
    runners: &Runners,
    tally: &mut Tally,
) -> TestResult {
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
    ] = row
    else {
        return Err(format!("bad golden row of {} fields", row.len()));
    };
    let doc = document(doc_ref, files)?;
    let name = String::from_utf8_lossy(go_type).into_owned();
    let run = runners
        .get(&name)
        .ok_or_else(|| format!("no Rust type for Go type {name}"))?;
    let rust = run(&doc);
    let what = format!("{name} {} {}", show(label), show(&doc));
    tally.rows += 1;
    tally.outputs += 2;
    let mut failures = Vec::new();
    if rust.jsoniter_err != *iter_err {
        failures.push(format!(
            "{what}: jsonutil.Unmarshal error {}, Go's is {}",
            show(&rust.jsoniter_err),
            show(iter_err)
        ));
    }
    if rust.std_err != *std_err {
        failures.push(format!(
            "{what}: json.Unmarshal error {}, Go's is {}",
            show(&rust.std_err),
            show(std_err)
        ));
    }
    let mut wants: Vec<Want> = Vec::with_capacity(6);
    for cell in [c1, c2, c3, c4, c5, c6] {
        let resolved = want(cell, &doc, &wants)?;
        wants.push(resolved);
    }
    // A Go string keeps the invalid UTF-8 that jsoniter reads; a Rust `String` holds U+FFFD
    // instead (see the crate docs), so only the jsoniter-decoded encodings can differ by it.
    let invalid_utf8 = from_utf8(&doc).is_err();
    for (i, (want, got)) in wants.iter().zip(&rust.encoded).enumerate() {
        tally.outputs += 1;
        if matches(want, got) {
            continue;
        }
        let string_gap = invalid_utf8
            && i < 3
            && got
                .as_ref()
                .is_ok_and(|bytes| matches(want, &Ok(as_go_invalid_bytes(bytes))));
        if string_gap {
            tally.string_gaps += 1;
        } else {
            failures.push(format!(
                "{what}: encoding {} is {}, Go's is {}",
                i + 1,
                describe_got(got),
                describe_want(want)
            ));
        }
    }
    if label.ends_with(STALE_ELEMENT) && !failures.is_empty() {
        tally.stale_elements += 1;
    } else {
        tally.failures.extend(failures);
    }
    Ok(())
}

/// Replays a golden file of `testdata/golden`.
fn replay(file: &str, runners: &Runners) -> TestResult<Tally> {
    let mut files = Vec::new();
    let mut tally = Tally::default();
    for row in golden_rows(file)? {
        match row.first().map(Vec::as_slice) {
            Some(b"#go") => {}
            Some(b"#root") => {
                let [_, root] = row.as_slice() else {
                    return Err(format!("bad #root row of {} fields", row.len()));
                };
                tally.roots.push(String::from_utf8_lossy(root).into_owned());
            }
            Some(b"#file") => {
                let (path, data) = corpus_file(&row)?;
                tally.files.push(path);
                files.push(data);
            }
            Some(_) | None => replay_row(&row, &files, runners, &mut tally)?,
        }
    }
    Ok(tally)
}

/// What a golden file may differ from Go by: the documented gaps.
#[derive(Clone, Copy)]
struct Gaps {
    /// Encodings that differ only by the `String` gap.
    strings: usize,
    /// [`STALE_ELEMENT`] rows that differ.
    stale_elements: usize,
}

/// No differences at all.
const NO_GAPS: Gaps = Gaps {
    strings: 0,
    stale_elements: 0,
};

/// Replays `file`, then checks that it differs from Go by exactly `gaps`, and that at least
/// `min_rows` documents were replayed.
fn check(file: &str, min_rows: usize, gaps: Gaps) -> TestResult<Tally> {
    let tally = replay(file, &runners()?)?;
    report(file, &tally.failures, 20)?;
    if tally.rows < min_rows {
        return Err(format!("{file}: only {} documents", tally.rows));
    }
    ensure_eq(
        &tally.string_gaps,
        &gaps.strings,
        &format!(
            "{file}: encodings that differ by the String gap ({} outputs)",
            tally.outputs
        ),
    )?;
    ensure_eq(
        &tally.stale_elements,
        &gaps.stale_elements,
        &format!("{file}: \"stale element\" cases that differ"),
    )?;
    Ok(tally)
}

/// The JSON files below `dir` (relative to the Go root, `/`-separated), appended to `out`: as the
/// generator's `filepath.WalkDir`, files whose name ends in `.json`, case-sensitively
/// (`strings.HasSuffix`), not following symbolic links to directories.
fn json_files(dir: &str, out: &mut Vec<String>) -> TestResult {
    let path = go_root()?.join(dir);
    let entries = fs::read_dir(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", path.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = format!("{dir}/{name}");
        let kind = entry
            .file_type()
            .map_err(|e| format!("reading {child}: {e}"))?;
        if kind.is_dir() {
            json_files(&child, out)?;
        } else if name.as_bytes().ends_with(b".json") {
            out.push(child);
        }
    }
    Ok(())
}

/// The fixture roots of `testdata/go_fixture_manifest.json` as the generator's
/// `fixtureRoots` reads them: distinct, without missing or empty ones.
fn fixture_roots() -> TestResult<BTreeSet<String>> {
    let path = workspace_root()?.join("testdata/go_fixture_manifest.json");
    let manifest: Value = serde_json::from_slice(&read(&path)?).map_err(|e| e.to_string())?;
    let runs = manifest
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("the fixture manifest has no runs")?;
    Ok(runs
        .iter()
        .filter_map(|run| run.get("fixture_root").and_then(Value::as_str))
        .filter(|root| !root.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Every `OpenRTB` document of the adapter, exchange, endpoint and first-party data test
/// corpora, and the native markup they carry. The corpus must be the one the goldens were made
/// from: every fixture root of the adapter test manifest is walked, and no JSON file was added
/// to or removed from a walked directory since.
#[test]
fn corpus() -> TestResult {
    let tally = check("corpus.tsv", 8100, NO_GAPS)?;
    let roots: BTreeSet<String> = tally.roots.iter().cloned().collect();
    let fixture_roots = fixture_roots()?;
    let missing: Vec<&String> = fixture_roots.difference(&roots).collect();
    if !missing.is_empty() {
        return Err(regenerate(&format!("fixture roots {missing:?} were added")));
    }
    let mut on_disk = Vec::new();
    for root in &tally.roots {
        json_files(root, &mut on_disk)?;
    }
    let on_disk: BTreeSet<String> = on_disk.into_iter().collect();
    let recorded: BTreeSet<String> = tally.files.iter().cloned().collect();
    let added: Vec<&String> = on_disk.difference(&recorded).collect();
    let removed: Vec<&String> = recorded.difference(&on_disk).collect();
    if !added.is_empty() || !removed.is_empty() {
        return Err(regenerate(&format!(
            "corpus files {added:?} were added and {removed:?} removed"
        )));
    }
    Ok(())
}

/// The openrtb module's testdata files.
#[test]
fn module() -> TestResult {
    check("module.tsv", 34, NO_GAPS).map(drop)
}

/// The encodings of the mutations that differ from Go by the documented `String` gap: the
/// three jsoniter-decoded encodings of each struct's string field set to the invalid UTF-8
/// byte `"\xff"` (82 structs). jsoniter keeps the byte and Go writes it escaped as U+FFFD; the
/// `String` field holds U+FFFD, written as its UTF-8 bytes (the same JSON string).
const MUTATION_STRING_GAPS: usize = 246;

/// The generated mutations of every ported type.
#[test]
fn mutations() -> TestResult {
    let gaps = Gaps {
        strings: MUTATION_STRING_GAPS,
        stale_elements: 0,
    };
    check("mutations.tsv", 20000, gaps).map(drop)
}

/// The [`STALE_ELEMENT`] cases of `adversarial.tsv` that differ from Go: one per struct field
/// that is a slice of structs (42), each `{"f":[A,B],"f":[B],"f":[{},{}]}`.
const ADVERSARIAL_STALE_ELEMENTS: usize = 42;

/// The adversarial documents of `testdata/go/adversarial.go`: merges on duplicate keys, slice
/// element edges, Unicode case folding of keys, malformed objects, `json.RawMessage` contents,
/// wrong values at every nested field path, and the nesting limit.
#[test]
fn adversarial() -> TestResult {
    let gaps = Gaps {
        strings: 0,
        stale_elements: ADVERSARIAL_STALE_ELEMENTS,
    };
    check("adversarial.tsv", 6000, gaps).map(drop)
}

/// Nesting far beyond Go's limit fails as at the limit, without exhausting the stack: Go
/// stops at depth 10001, so a decoder that recursed further would crash a Rust thread.
#[test]
fn deep_nesting() -> TestResult {
    let rows = golden_rows("adversarial.tsv")?;
    let limit = rows
        .iter()
        .find(|row| row.get(2).is_some_and(|label| label == b"depth ext 10001"))
        .ok_or("adversarial.tsv has no \"depth ext 10001\" row")?;
    let (Some(iter_err), Some(std_err)) = (limit.get(3), limit.get(7)) else {
        return Err("bad \"depth ext 10001\" row".to_owned());
    };
    let depth = 1_000_000;
    let mut doc = b"{\"ext\":".to_vec();
    doc.extend(repeat_n(b'[', depth));
    doc.extend(repeat_n(b']', depth));
    doc.push(b'}');
    let runner = runners()?
        .get("openrtb2.BidRequest")
        .copied()
        .ok_or("no runner for openrtb2.BidRequest")?;
    // A thread with the 2 MiB stack Rust gives spawned threads (and tokio its workers).
    let outcome = thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let o = runner(&doc);
            (o.jsoniter_err, o.std_err)
        })
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|panic| format!("the decoder thread panicked: {panic:?}"))?;
    ensure_eq(&outcome.0, iter_err, "jsonutil.Unmarshal error")?;
    ensure_eq(&outcome.1, std_err, "json.Unmarshal error")
}
