//! Replays the decoding goldens that `testdata/go` writes (its `main.go` documents the format):
//! every decoder error and every encoding must match Go byte for byte.

use core::str::from_utf8;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use pbs_gojson::{GoEncode, JsoniterDecode, StdDecode, gotype::go_type_name, jsonutil, stdjson};
use pbs_openrtb_ext::openrtb_ext;
use serde_json::Value;

use super::{
    index::{TYPES, for_each_type},
    support::{
        TestResult, as_go_invalid_bytes, ensure_eq, fnv64, go_root, golden_rows, read, report,
        show, workspace_root,
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
fn encode<T: GoEncode>(v: &T) -> [Encoded; 3] {
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

/// Counts from replaying one golden file.
#[derive(Default)]
struct Tally {
    /// Documents replayed.
    rows: usize,
    /// Outputs compared (two error texts and six encodings per document).
    outputs: usize,
    /// The rows (`Type label`) with encodings that differ only as the documented `String` gap
    /// allows, and how many.
    string_gaps: BTreeMap<String, usize>,
    /// The rows with encodings that differ only as [`POINTER_TO_SLICE_GAP`] allows, and how
    /// many.
    pointer_to_slice_gaps: BTreeMap<String, usize>,
    /// The Go types of the rows.
    types: BTreeSet<String>,
    /// Every other difference.
    failures: Vec<String>,
    /// The directories of the `#root` rows.
    roots: Vec<String>,
    /// The paths of the `#file` rows.
    files: Vec<String>,
}

/// The message for a corpus that changed after the goldens were written.
fn regenerate(what: &str) -> String {
    format!(
        "{what} after the goldens were written; regenerate them with \
         `GOTOOLCHAIN=go1.26.2 go run .` in crates/openrtb-ext/testdata/go"
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

/// The mutations (`Type label`) where `pbs_gojson` differs from Go (reported for a fix there):
/// decoding a non-`null` literal into a pointer to a slice (`ImpExtConcert.Sizes`, `*[][]int`)
/// with `encoding/json` is a type error that leaves the pointer to a nil slice in Go, and to an
/// empty one in Rust (`Vec`'s `StdDecode::std_literal_option` allocates the slice). Each row's
/// three `json.Unmarshal`-decoded encodings write `[]` where Go's write `null`.
const POINTER_TO_SLICE_GAP: [&str; 2] = [
    "openrtb_ext.ImpExtConcert sizes wrong 1",
    "openrtb_ext.ImpExtConcert sizes wrong 2",
];

/// Whether `got` is `want` but for [`POINTER_TO_SLICE_GAP`]'s `[]` for `null`.
fn pointer_to_slice_gap(want: &Want, got: &Encoded) -> bool {
    let Ok(bytes) = got else {
        return false;
    };
    let empty: &[u8] = br#""sizes":[]"#;
    let Some(at) = bytes.windows(empty.len()).position(|w| w == empty) else {
        return false;
    };
    let (before, after) = bytes.split_at(at);
    let mut go = before.to_vec();
    go.extend_from_slice(br#""sizes":null"#);
    go.extend_from_slice(after.get(empty.len()..).unwrap_or_default());
    matches(want, &Ok(go))
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
    let row_name = format!("{name} {}", String::from_utf8_lossy(label));
    let what = format!("{name} {} {}", show(label), show(&doc));
    tally.rows += 1;
    tally.outputs += 2;
    let _new = tally.types.insert(name);
    if rust.jsoniter_err != *iter_err {
        tally.failures.push(format!(
            "{what}: jsonutil.Unmarshal error {}, Go's is {}",
            show(&rust.jsoniter_err),
            show(iter_err)
        ));
    }
    if rust.std_err != *std_err {
        tally.failures.push(format!(
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
            *tally.string_gaps.entry(row_name.clone()).or_default() += 1;
        } else if i >= 3
            && POINTER_TO_SLICE_GAP.contains(&row_name.as_str())
            && pointer_to_slice_gap(want, got)
        {
            *tally
                .pointer_to_slice_gaps
                .entry(row_name.clone())
                .or_default() += 1;
        } else {
            tally.failures.push(format!(
                "{what}: encoding {} is {}, Go's is {}",
                i + 1,
                describe_got(got),
                describe_want(want)
            ));
        }
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

/// Each of `rows` (`Type label`) with its three encodings of one decoder.
fn three_each(rows: &[&str]) -> BTreeMap<String, usize> {
    rows.iter().map(|&row| (row.to_owned(), 3)).collect()
}

/// Replays `file`, then checks that it differs from Go only by the three jsoniter-decoded
/// encodings of the `string_gaps` rows (the documented `String` gap) and the three
/// `json.Unmarshal`-decoded encodings of the `pointer_to_slice_gaps` rows
/// ([`POINTER_TO_SLICE_GAP`]), and that at least `min_rows` documents were replayed.
fn check(
    file: &str,
    min_rows: usize,
    string_gaps: &[&str],
    pointer_to_slice_gaps: &[&str],
) -> TestResult<Tally> {
    let tally = replay(file, &runners()?)?;
    report(file, &tally.failures, 20)?;
    if tally.rows < min_rows {
        return Err(format!("{file}: only {} documents", tally.rows));
    }
    ensure_eq(
        &tally.string_gaps,
        &three_each(string_gaps),
        &format!(
            "{file}: encodings that differ by the String gap ({} outputs)",
            tally.outputs
        ),
    )?;
    ensure_eq(
        &tally.pointer_to_slice_gaps,
        &three_each(pointer_to_slice_gaps),
        &format!("{file}: encodings that differ by the pointer-to-slice gap"),
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

/// Every Prebid extension object of the adapter, exchange, endpoint and first-party data test
/// corpora. The corpus must be the one the goldens were made from: every fixture root of the
/// adapter test manifest is walked, and no JSON file was added to or removed from a walked
/// directory since.
#[test]
fn corpus() -> TestResult {
    let tally = check("corpus.tsv", 4000, &[], &[])?;
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
    // Every bidder params type that some adapter fixture holds is exercised.
    for name in ["openrtb_ext.ExtImpAppnexus", "openrtb_ext.ExtImpRubicon"] {
        if !tally.types.contains(name) {
            return Err(format!("corpus.tsv has no {name} document"));
        }
    }
    Ok(())
}

/// The mutations (`Type label`) whose jsoniter-decoded encodings differ from Go by the
/// documented `String` gap: the string set to the invalid UTF-8 byte `"\xff"` (a named string
/// type's top-level value, and the first field of each string type that gets edge values).
/// jsoniter keeps the byte and Go writes it escaped as `�`; the `String` holds U+FFFD,
/// written as its UTF-8 bytes (the same JSON string). (`ExtImpAppnexus`'s `member` edge is no
/// gap: Go's jsoniter-decoded value holds U+FFFD there too.)
const MUTATION_STRING_GAPS: [&str; 10] = [
    "openrtb_ext.ExtAppPrebid source edge 12",
    "openrtb_ext.ExtBidPrebid type edge 12",
    "openrtb_ext.BidType value 14",
    "openrtb_ext.TargetingKey value 14",
    "openrtb_ext.BidderName value 14",
    "openrtb_ext.ImpExtConcert site edge 12",
    "openrtb_ext.ExtResponseSyncData status edge 12",
    "openrtb_ext.ExtUserSync type edge 12",
    "openrtb_ext.CookieStatus value 14",
    "openrtb_ext.UserSyncType value 14",
];

/// The adversarial review's documents (`testdata/go/review.go`): the values each `UnmarshalJSON`
/// method tells apart, nested up to four levels deep in the types that hold them; grids of
/// price granularity, interstitial, AMP, keyword, deal tier and multibid documents; every edge
/// value at every `jsonutil.StringInt`, `jsonutil.IntString` and `json.Number` field; and
/// random documents (one in six truncated or corrupted) of every type JSON decodes into.
#[test]
fn review() -> TestResult {
    let tally = check("review.tsv", 4800, &[], &[])?;
    for name in [
        "openrtb_ext.ExtRequest",
        "openrtb_ext.ExtDevice",
        "openrtb_ext.ImpExtGoldbach",
        "openrtb_ext.ExtImpAppnexus",
        "openrtb_ext.ExtImpRubicon",
    ] {
        if !tally.types.contains(name) {
            return Err(format!("review.tsv has no {name} document"));
        }
    }
    Ok(())
}

/// The generated mutations of every type JSON decodes into, which must cover every one of
/// them.
#[test]
fn mutations() -> TestResult {
    let tally = check(
        "mutations.tsv",
        19000,
        &MUTATION_STRING_GAPS,
        &POINTER_TO_SLICE_GAP,
    )?;
    let want: BTreeSet<String> = TYPES.iter().map(|&name| name.to_owned()).collect();
    ensure_eq(&tally.types, &want, "the Go types of mutations.tsv")
}
