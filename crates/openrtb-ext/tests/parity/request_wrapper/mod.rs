//! The request wrapper and the conversions against Go's goldens.
//!
//! Go: `openrtb_ext/request_wrapper.go`, `convert_up.go` and `convert_down.go`. The goldens
//! `testdata/go/request_wrapper` writes with Go's `RequestWrapper` (every bid request of
//! prebid-server's test corpora, generated mutations and the review's requests, run through scripts
//! of wrapper operations: getters, setters, rebuilds, the conversions, the `Clear*` functions, the
//! impression wrappers, the clones and the review's sequences, such as exchange/utils.go's
//! per-bidder request), with the observation of every step, replayed through the crate's public
//! API.
//!
//! Each row's request is decoded with `pbs_jsonutil::unmarshal`, as Go decodes it with
//! `jsonutil.Unmarshal`, and every script starts from a new wrapper of it. Observations compare
//! part by part (`obs`): error texts exactly where Prebid wrote them and as `json.error-texts`
//! where the library did; JSON by its tokens, a string by its text and a number by its value,
//! each other spelling a lexical class (`json.html-escape`, `json.float-format`), whitespace
//! never (`json.raw-not-compacted` is restored); ext maps member by member. Every other
//! difference must fall in a ledger entry:
//!
//! * a fix of the document that makes the replay match, for a row whose document `serde_json` reads
//!   otherwise: bytes that are not UTF-8 read as U+FFFD (`json.invalid-utf8`), `-0` written `0`
//!   (`json.negative-zero-int`), keys respelled as jsoniter matches them and repeated members
//!   dropped (`json.key-case`, `json.duplicate-keys`); the JSON Go wrote gets the same rewrites, so
//!   that a raw member Go kept as the document held it compares with the port's;
//! * where Go wrote a raw value as nothing, the port's null (`json.nil-raw-written`), or a value of
//!   the port's that holds the null Go could not write (`json.ext-null-member`: an ext map with a
//!   null member, which Go's rebuild writes as nothing and stores as bytes that are not JSON, so
//!   that Go fails to read them later in the script);
//! * after an ext that failed to decode, what Go's decoders left in the wrapper and the port's do
//!   not (`json.partial-results`): the port's observation must hold no more than Go's, until Go
//!   reports the failure, after which the steps go on from Go's partial values;
//! * a struct the serde derive reads from an array, where Go fails (`json.struct-from-array`), and
//!   what follows in the script;
//! * after a value JSON cannot give (`special`), the step Go fails to write and what follows
//!   (`json.non-finite`, `json.raw-not-json`), and a raw ext member that is not JSON, which the
//!   port's ext maps cannot hold (`json.raw-not-json`).
//!
//! Regenerate the goldens with `GOTOOLCHAIN=go1.26.2 go run ./request_wrapper` in
//! `crates/openrtb-ext/testdata/go`; the corpora are read from the Go root (`$PBS_GO_ROOT`, else
//! `.upstream`, which `scripts/fetch-upstream.sh` makes).

mod obs;
mod runner;

use core::str::from_utf8;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read as _,
};

use flate2::read::GzDecoder;
use pbs_jsonutil::unmarshal;
use pbs_openrtb::openrtb2;
use serde_json::{Value, value::RawValue};

use self::{
    obs::{EXT_NULL_MEMBER, LONE_SURROGATES_RAW, NON_FINITE, Part},
    runner::{Outcome, RAW_NOT_JSON, Runner},
};
use crate::{
    go_quote::unquote,
    serde_goldens::without_negative_zeros,
    shapes::{
        ERROR_TEXTS, FLOAT_FORMAT, HTML_ESCAPE, KEY_CASE, Node, PARTIAL_RESULTS, RESPELLINGS,
        Respelling, STRUCT_FROM_ARRAY, nil_raw_as_null, respelled_keys,
    },
    support::{TestResult, crate_dir, ensure_eq, fnv64, go_root, read, show, workspace_root},
};

/// `json.invalid-utf8`: `serde_json` rejects bytes that are not UTF-8 in a string, which Go
/// reads as U+FFFD.
const INVALID_UTF8: &str = "json.invalid-utf8";

/// `json.negative-zero-int`: `serde_json` reads `-0` as a float, which no integer field takes.
const NEGATIVE_ZERO_INT: &str = "json.negative-zero-int";

/// The message for a corpus that changed after the goldens were written.
fn regenerate(what: &str) -> String {
    format!(
        "{what} after the goldens were written; regenerate them with \
         `GOTOOLCHAIN=go1.26.2 go run ./request_wrapper` in crates/openrtb-ext/testdata/go"
    )
}

/// A cell of a row: what Go observed in one step.
#[derive(Debug, Clone)]
enum Cell {
    /// The step's observation.
    Observed(Vec<Part>),
    /// Go panicked with this text.
    Panicked(String),
    /// A step after a panic, which Go did not run.
    Skipped,
}

/// A request row: its document, where it comes from, its suite and Go's cells.
#[derive(Debug)]
struct Row {
    /// The document.
    doc: Vec<u8>,
    /// The corpus file and label, for failure messages.
    place: String,
    /// The suite of scripts the row runs.
    suite: String,
    /// One cell per step of the suite's scripts.
    cells: Vec<Cell>,
}

/// A golden file: its corpus, scripts and suites, and its request rows.
#[derive(Debug, Default)]
struct Golden {
    /// The directories of the `#root` rows.
    roots: Vec<String>,
    /// The path of each `#file` row.
    files: Vec<String>,
    /// The steps of each script.
    scripts: BTreeMap<String, Vec<String>>,
    /// The scripts of each suite.
    suites: BTreeMap<String, Vec<String>>,
    /// The request rows.
    rows: Vec<Row>,
}

/// A field as text.
fn text(field: &[u8]) -> TestResult<String> {
    from_utf8(field)
        .map(str::to_owned)
        .map_err(|e| format!("field {}: {e}", show(field)))
}

/// The fields after the first two of a `#` row, as text.
fn names(row: &[Vec<u8>]) -> TestResult<Vec<String>> {
    row.iter().skip(2).map(|field| text(field)).collect()
}

/// The document of a row: `'` and its bytes, or `@file:offset:length` into a corpus file.
fn document(field: &[u8], files: &[(String, Vec<u8>)]) -> TestResult<(Vec<u8>, String)> {
    if let Some(inline) = field.strip_prefix(b"'") {
        return Ok((inline.to_vec(), String::new()));
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
    let (path, data) = files.get(*file).ok_or_else(bad)?;
    let doc = data.get(*offset..end).ok_or_else(bad)?;
    Ok((doc.to_vec(), path.clone()))
}

/// The cells of a row, `=N` resolved to cell N's observation.
fn cells(fields: &[Vec<u8>]) -> TestResult<Vec<Cell>> {
    let mut cells: Vec<Cell> = Vec::with_capacity(fields.len());
    for field in fields {
        let cell = match field.split_first() {
            Some((b'\'', obs)) => Cell::Observed(obs::parse(obs)?),
            Some((b'!', panic)) => Cell::Panicked(String::from_utf8_lossy(panic).into_owned()),
            Some((b'-', [])) => Cell::Skipped,
            Some((b'=', index)) => from_utf8(index)
                .ok()
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| cells.get(index.checked_sub(1)?))
                .filter(|cell| matches!(cell, Cell::Observed(_)))
                .cloned()
                .ok_or_else(|| format!("bad cell reference {}", show(field)))?,
            _ => return Err(format!("bad cell {}", show(field))),
        };
        cells.push(cell);
    }
    Ok(cells)
}

/// Reads a gzip-compressed golden file of `testdata/go/request_wrapper`, checking its corpus
/// files against the hashes Go recorded.
fn golden(file: &str) -> TestResult<Golden> {
    let path = crate_dir().join("testdata/go/request_wrapper").join(file);
    let mut data = Vec::new();
    let _len = GzDecoder::new(read(&path)?.as_slice())
        .read_to_end(&mut data)
        .map_err(|e| format!("decompressing {}: {e}", path.display()))?;
    let content = from_utf8(&data).map_err(|e| format!("{}: not ASCII: {e}", path.display()))?;
    let mut golden = Golden::default();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut previous: Option<(Vec<u8>, String)> = None;
    for (n, line) in content.lines().enumerate() {
        let row = line
            .split('\t')
            .map(|field| unquote(field).map_err(|e| format!("{file}:{}: {e}", n + 1)))
            .collect::<TestResult<Vec<Vec<u8>>>>()?;
        let second = row.get(1).map(|field| text(field)).transpose()?;
        match (row.first().map(Vec::as_slice), second) {
            (Some(b"#go"), _) => {}
            (Some(b"#root"), Some(root)) => golden.roots.push(root),
            (Some(b"#file"), Some(corpus_path)) => {
                let corpus = read(&go_root()?.join(&corpus_path))?;
                let recorded = row.get(2).map_or(&[][..], Vec::as_slice);
                if format!("{:016x}", fnv64(&corpus)).as_bytes() != recorded {
                    return Err(regenerate(&format!("{corpus_path} changed")));
                }
                golden.files.push(corpus_path.clone());
                files.push((corpus_path, corpus));
            }
            (Some(b"#script"), Some(name)) => {
                let _previous = golden.scripts.insert(name, names(&row)?);
            }
            (Some(b"#suite"), Some(name)) => {
                let _previous = golden.suites.insert(name, names(&row)?);
            }
            _ => {
                let [doc_ref, label, suite, fields @ ..] = row.as_slice() else {
                    return Err(format!("{file}:{}: too few fields", n + 1));
                };
                let (doc, place) = if doc_ref.as_slice() == b"^" {
                    previous
                        .clone()
                        .ok_or_else(|| format!("{file}:{}: ^ without a row before", n + 1))?
                } else {
                    let (doc, corpus) = document(doc_ref, &files)?;
                    let place = format!("{corpus} {}", String::from_utf8_lossy(label));
                    previous = Some((doc.clone(), place.clone()));
                    (doc, place)
                };
                let cells = cells(fields).map_err(|e| format!("{file}:{}: {e}", n + 1))?;
                golden.rows.push(Row {
                    doc,
                    place,
                    suite: text(suite)?,
                    cells,
                });
            }
        }
    }
    Ok(golden)
}

/// The JSON files below `dir` (relative to the Go root, `/`-separated), appended to `out`: as the
/// generator's `filepath.WalkDir`, files whose name ends in `.json`, not following symbolic
/// links to directories.
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

/// The fixture roots of `testdata/go_fixture_manifest.json` as the generator reads them:
/// distinct, without missing or empty ones.
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

/// Checks that the corpus is the one the goldens were made from: every fixture root of the
/// adapter test manifest is walked, and no JSON file was added to or removed from a walked
/// directory since.
fn check_corpus(golden: &Golden) -> TestResult {
    let roots: BTreeSet<String> = golden.roots.iter().cloned().collect();
    let missing: Vec<String> = fixture_roots()?.difference(&roots).cloned().collect();
    if !missing.is_empty() {
        return Err(regenerate(&format!("fixture roots {missing:?} were added")));
    }
    let mut on_disk = Vec::new();
    for root in &golden.roots {
        json_files(root, &mut on_disk)?;
    }
    let on_disk: BTreeSet<String> = on_disk.into_iter().collect();
    let recorded: BTreeSet<String> = golden.files.iter().cloned().collect();
    let added: Vec<&String> = on_disk.difference(&recorded).collect();
    let removed: Vec<&String> = recorded.difference(&on_disk).collect();
    if !added.is_empty() || !removed.is_empty() {
        return Err(regenerate(&format!(
            "corpus files {added:?} were added and {removed:?} removed"
        )));
    }
    Ok(())
}

/// What the replay of a golden file found.
#[derive(Debug, Default)]
struct Tally {
    /// The request rows replayed.
    rows: usize,
    /// The steps whose observation was compared.
    steps: usize,
    /// The steps where Go panicked on a write into a nil map, and the port returned that error.
    nil_map_panics: usize,
    /// The ledger entries the differences fall in, with the number of steps (or, for a fix of
    /// the document, rows) of each.
    classes: BTreeMap<&'static str, usize>,
    /// Every difference outside the ledger.
    failures: Vec<String>,
}

/// What the replay of one row with one document found.
#[derive(Debug, Default)]
struct RowTally {
    /// The steps compared.
    steps: usize,
    /// The nil map panics both sides had.
    nil_map_panics: usize,
    /// The ledger entries of the steps' differences, with the number of steps of each.
    classes: BTreeMap<&'static str, usize>,
    /// The differences outside the ledger.
    failures: Vec<String>,
}

/// `json.struct-from-array` when Go fails to read a struct from an array and the port reads it
/// (the serde derive reads a struct from a sequence).
fn struct_from_array(port: &[Part], go: &[Part]) -> Option<&'static str> {
    port.iter()
        .zip(go)
        .any(|(p, g)| p.error().is_empty() && g.error().contains("expect { or n, but found ["))
        .then_some(STRUCT_FROM_ARRAY)
}

/// What happened earlier in a script, which explains a later step's difference from Go.
#[derive(Debug, Default)]
struct History {
    /// A ledger entry that explains every later difference in the script: a struct read from
    /// an array, which leaves the port's wrapper with what Go's never read, or a rebuild that
    /// left Go's request with bytes that are not JSON where a nil raw member was written as
    /// nothing.
    taint: Option<&'static str>,
    /// Go returned a decoding error in an earlier step and the port failed to decode an ext
    /// too: Go's wrapper then holds what its decoders read before the failure, and the steps
    /// that use it go on from there.
    failed_in_go: bool,
}

/// Why a step that differs from Go is a ledgered difference, given what happened earlier in its
/// script: the value JSON cannot give that a step set; a struct read from an array; after an ext
/// that failed to decode, an observation that holds no more than Go's, or, once Go reported the
/// failure, any observation.
fn explained(
    runner: &Runner,
    history: &History,
    port: &[Part],
    go: &[Part],
) -> Option<&'static str> {
    if let Some(&class) = runner.tainted().iter().next() {
        return Some(class);
    }
    if let Some(class) = history.taint.or_else(|| struct_from_array(port, go)) {
        return Some(class);
    }
    (!runner.failed().is_empty() && (history.failed_in_go || obs::partial(port, go)))
        .then_some(PARTIAL_RESULTS)
}

/// Replays a row's scripts on `request`, the row's document decoded, against `cells`.
fn replay_row(
    golden: &Golden,
    row: &Row,
    cells: &[Cell],
    request: &openrtb2::BidRequest,
) -> TestResult<RowTally> {
    let mut tally = RowTally::default();
    let scripts = golden
        .suites
        .get(&row.suite)
        .ok_or_else(|| format!("{}: unknown suite {}", row.place, row.suite))?;
    let mut cells = cells.iter();
    for script in scripts {
        let steps = golden
            .scripts
            .get(script)
            .ok_or_else(|| format!("unknown script {script}"))?;
        let mut runner = Runner::new(request.clone());
        let mut history = History::default();
        let mut stopped = false;
        for (i, step) in steps.iter().enumerate() {
            let cell = cells
                .next()
                .ok_or_else(|| format!("{}: fewer cells than steps", row.place))?;
            let at = || format!("{}: script {script} step {i} `{step}`", row.place);
            if stopped {
                if !matches!(cell, Cell::Skipped) {
                    tally
                        .failures
                        .push(format!("{}: the port stopped, Go did not", at()));
                }
                continue;
            }
            let outcome = runner.step(step)?;
            tally.steps += 1;
            match (outcome, cell) {
                (Outcome::Observed(port), Cell::Observed(go)) => {
                    match obs::compare(&port, go) {
                        Ok(classes) => {
                            // Go's rebuild stored bytes that are not JSON (an ext map with a nil
                            // member it wrote as nothing), which Go then fails to read.
                            if step.starts_with("rebuild") && classes.contains(&EXT_NULL_MEMBER) {
                                history.taint = Some(EXT_NULL_MEMBER);
                            }
                            for class in classes {
                                *tally.classes.entry(class).or_insert(0) += 1;
                            }
                        }
                        Err(e) => match explained(&runner, &history, &port, go) {
                            Some(class) => {
                                *tally.classes.entry(class).or_insert(0) += 1;
                                if class == STRUCT_FROM_ARRAY {
                                    history.taint = Some(class);
                                }
                            }
                            None => tally.failures.push(format!("{}: {e}", at())),
                        },
                    }
                    if !runner.failed().is_empty() && go.iter().any(|p| !p.error().is_empty()) {
                        history.failed_in_go = true;
                    }
                }
                (Outcome::Panicked(port), Cell::Panicked(go)) if port == *go => {
                    tally.nil_map_panics += 1;
                    stopped = true;
                }
                (Outcome::Inexpressible(class), _) => {
                    *tally.classes.entry(class).or_insert(0) += 1;
                    for _skipped in steps.iter().skip(i + 1) {
                        let _cell = cells.next();
                    }
                    break;
                }
                (outcome, cell) => tally
                    .failures
                    .push(format!("{}: port {outcome:?}, Go {cell:?}", at())),
            }
        }
    }
    if cells.next().is_some() {
        tally
            .failures
            .push(format!("{}: more cells than steps", row.place));
    }
    Ok(tally)
}

/// The keys of every object in Go's observations of a row that the row's document does not
/// spell so: what Go's decoder matched the document's keys to where it matched them whatever
/// their case (a raw member Go wrote as the document held it gives the document's keys).
fn go_keys(row: &Row) -> BTreeSet<String> {
    let mut doc_keys = BTreeSet::new();
    if let Some(node) = Node::parse(&String::from_utf8_lossy(&row.doc)) {
        node.keys_into(&mut doc_keys);
    }
    let mut keys = BTreeSet::new();
    for cell in &row.cells {
        let Cell::Observed(parts) = cell else {
            continue;
        };
        for part in parts {
            if let Part::Json(json) = part
                && let Some(node) = Node::parse(&nil_raw_as_null(json))
            {
                node.keys_into(&mut keys);
            }
        }
    }
    keys.retain(|key| !doc_keys.contains(key));
    keys
}

/// JSON as [`Node::write`] writes it (compact), or as it is when it is not JSON.
fn normalized(text: &str) -> String {
    Node::parse(text).map_or_else(
        || text.to_owned(),
        |node| {
            let mut out = String::new();
            node.write(&mut out);
            out
        },
    )
}

/// A fix of a row's document: the fixed document, the ledger entries of what it changes, and
/// the rewrites it makes, which the JSON Go wrote gets too, so that a raw member Go kept as the
/// document held it compares with the port's, read from the fixed document.
#[derive(Debug)]
struct Fix {
    /// The fixed document.
    doc: Vec<u8>,
    /// The ledger entries of what the fix changes.
    classes: Vec<&'static str>,
    /// `-0` is written `0`.
    zeros: bool,
    /// Keys are respelled as Go's decoder matched them.
    respelling: Option<Respelling>,
}

impl Fix {
    /// `text` (JSON Go wrote) with the fix's rewrites, compact.
    fn rewrite(&self, text: &str, keys: &BTreeSet<String>) -> String {
        let mut out = normalized(text);
        if self.zeros
            && let Some(fixed) = without_negative_zeros(&out)
        {
            out = fixed;
        }
        if let Some(mode) = self.respelling
            && let Some(respelled) = respelled_keys(&out, keys, mode)
        {
            out = respelled.doc;
        }
        out
    }

    /// Go's cells with the fix's rewrites in their JSON and ext members.
    fn cells(&self, cells: &[Cell], keys: &BTreeSet<String>) -> Vec<Cell> {
        if !self.zeros && self.respelling.is_none() {
            return cells.to_vec();
        }
        cells
            .iter()
            .map(|cell| match cell {
                Cell::Observed(parts) => Cell::Observed(
                    parts
                        .iter()
                        .map(|part| match part {
                            Part::Json(text) => Part::Json(self.rewrite(text, keys)),
                            Part::Map(members) => Part::Map(
                                members
                                    .iter()
                                    .map(|(key, value)| {
                                        (key.clone(), value.as_ref().map(|v| self.rewrite(v, keys)))
                                    })
                                    .collect(),
                            ),
                            other @ (Part::Err(_) | Part::MarshalErr(_) | Part::Text(_)) => {
                                other.clone()
                            }
                        })
                        .collect(),
                ),
                other @ (Cell::Panicked(_) | Cell::Skipped) => other.clone(),
            })
            .collect()
    }
}

/// The fixes to replay a row with when its document does not replay as it is, in turn: bytes
/// that are not UTF-8 read as U+FFFD, `-0` written `0` and keys respelled as jsoniter matches
/// them.
fn fixes(row: &Row, keys: &BTreeSet<String>) -> Vec<Fix> {
    let mut out = Vec::new();
    let lossy = from_utf8(&row.doc).is_err();
    let text = String::from_utf8_lossy(&row.doc).into_owned();
    let base: Vec<&'static str> = if lossy {
        vec![INVALID_UTF8]
    } else {
        Vec::new()
    };
    let mut bases = vec![(text.clone(), base.clone(), false)];
    if let Some(fixed) = without_negative_zeros(&text) {
        let mut classes = base;
        classes.push(NEGATIVE_ZERO_INT);
        bases.push((fixed, classes, true));
    }
    for (doc, classes, zeros) in bases {
        if !classes.is_empty() {
            out.push(Fix {
                doc: doc.clone().into_bytes(),
                classes: classes.clone(),
                zeros,
                respelling: None,
            });
        }
        for mode in RESPELLINGS {
            if let Some(respelled) = respelled_keys(&doc, keys, mode) {
                let mut all = classes.clone();
                all.extend(respelled.classes);
                out.push(Fix {
                    doc: respelled.doc.into_bytes(),
                    classes: all,
                    zeros,
                    respelling: Some(mode),
                });
            }
        }
    }
    out
}

/// The replay of a row with `doc` against `cells`: `Ok` when it matches Go but for ledgered
/// differences, else what differs.
fn replay_doc(
    golden: &Golden,
    row: &Row,
    doc: &[u8],
    cells: &[Cell],
) -> TestResult<Result<RowTally, String>> {
    let request = match unmarshal::<openrtb2::BidRequest>(doc) {
        Ok(request) => request,
        Err(e) => return Ok(Err(format!("{}: decoding the request: {e}", row.place))),
    };
    let row_tally = replay_row(golden, row, cells, &request)?;
    Ok(if row_tally.failures.is_empty() {
        Ok(row_tally)
    } else {
        Err(row_tally.failures.join("\n"))
    })
}

/// Replays the request rows of a golden file.
fn replay(golden: &Golden) -> TestResult<Tally> {
    let mut tally = Tally::default();
    for row in &golden.rows {
        let mut replayed = None;
        let first = match replay_doc(golden, row, &row.doc, &row.cells)? {
            Ok(row_tally) => {
                replayed = Some((row_tally, Vec::new()));
                String::new()
            }
            Err(first) => first,
        };
        if replayed.is_none() {
            let keys = go_keys(row);
            for fix in fixes(row, &keys) {
                let cells = fix.cells(&row.cells, &keys);
                if let Ok(row_tally) = replay_doc(golden, row, &fix.doc, &cells)? {
                    replayed = Some((row_tally, fix.classes));
                    break;
                }
            }
        }
        let Some((row_tally, fixes)) = replayed else {
            tally.failures.push(first);
            continue;
        };
        tally.rows += 1;
        tally.steps += row_tally.steps;
        tally.nil_map_panics += row_tally.nil_map_panics;
        for (class, n) in row_tally.classes {
            *tally.classes.entry(class).or_insert(0) += n;
        }
        for class in fixes {
            *tally.classes.entry(class).or_insert(0) += 1;
        }
    }
    Ok(tally)
}

/// Fails with the first failures of a replay, when it replayed fewer than `min_rows` rows, and
/// unless the differences fall in exactly the ledger entries `classes`.
fn report(file: &str, tally: &Tally, min_rows: usize, classes: &[&str]) -> TestResult {
    if !tally.failures.is_empty() {
        let shown: Vec<&str> = tally.failures.iter().take(12).map(String::as_str).collect();
        return Err(format!(
            "{file}: {} rows differ from Go outside the ledger (classes so far {:?}); the first \
             {}:\n{}",
            tally.failures.len(),
            tally.classes,
            shown.len(),
            shown.join("\n")
        ));
    }
    if tally.rows < min_rows {
        return Err(format!("{file}: only {} requests", tally.rows));
    }
    ensure_eq(
        &tally.classes.keys().copied().collect::<Vec<_>>(),
        &classes.to_vec(),
        &format!(
            "{file}: the ledger entries the differences fall in ({} steps: {:?})",
            tally.steps, tally.classes
        ),
    )
}

/// Every bid request of the adapter, exchange, endpoint and first-party data test corpora. The
/// error texts of the library differ (the requests whose ext members fail to decode, such as
/// `exchange/exchangetest/request-imp-ext-prebid-filtering.json`'s `is_rewarded_inventory`), as
/// do the spellings of strings and floats in rebuilt exts, what jsoniter leaves of an ext that
/// fails to decode, the keys it matches whatever their case (`withCategory`,
/// `durationRangeSec`, `buyerUid`, `deviceType`), and the exts with a null member (the pangle
/// and vungle bodies' `"prebid":null`), which Go's rebuild writes as nothing.
#[test]
fn corpus() -> TestResult {
    let golden = golden("corpus.tsv.gz")?;
    check_corpus(&golden)?;
    let tally = replay(&golden)?;
    report(
        "corpus.tsv.gz",
        &tally,
        4900,
        &[
            ERROR_TEXTS,
            EXT_NULL_MEMBER,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            KEY_CASE,
            PARTIAL_RESULTS,
        ],
    )?;
    ensure_eq(&tally.nil_map_panics, &0, "corpus.tsv.gz: nil map panics")
}

/// The generated requests, and the setter scripts. Go panics on a write into a nil map (a
/// member set after `SetExt(nil)`, or into an ext that is the JSON `null`) in 72 steps, where
/// the port's rebuild returns the panic's text as an error. The members given as arrays where a
/// struct is expected (`"prebid":[]`) decode in the port, and a member set to bytes that are not
/// JSON cannot be set at all.
#[test]
fn mutations() -> TestResult {
    let golden = golden("mutations.tsv.gz")?;
    let tally = replay(&golden)?;
    report(
        "mutations.tsv.gz",
        &tally,
        220,
        &[
            ERROR_TEXTS,
            EXT_NULL_MEMBER,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            PARTIAL_RESULTS,
            RAW_NOT_JSON,
            STRUCT_FROM_ARRAY,
        ],
    )?;
    ensure_eq(
        &tally.nil_map_panics,
        &72,
        "mutations.tsv.gz: nil map panics",
    )
}

/// The review suites: the review requests through the corpus and review scripts, and every
/// corpus request through the review corpus scripts (see `testdata/go/request_wrapper/review.go`).
#[test]
fn review() -> TestResult {
    let golden = golden("review.tsv.gz")?;
    check_corpus(&golden)?;
    let tally = replay(&golden)?;
    report(
        "review.tsv.gz",
        &tally,
        5200,
        &[
            ERROR_TEXTS,
            EXT_NULL_MEMBER,
            FLOAT_FORMAT,
            HTML_ESCAPE,
            KEY_CASE,
            LONE_SURROGATES_RAW,
            NEGATIVE_ZERO_INT,
            NON_FINITE,
            PARTIAL_RESULTS,
            RAW_NOT_JSON,
        ],
    )
}

/// The observations of the steps of `script` on the row of the golden `file` labelled `label`:
/// each step with Go's cell and the port's outcome, in order.
fn script_steps(file: &str, label: &str, script: &str) -> TestResult<Vec<(String, Cell, Outcome)>> {
    let golden = golden(file)?;
    let runs = |row: &&Row| {
        golden
            .suites
            .get(&row.suite)
            .is_some_and(|scripts| scripts.iter().any(|name| name == script))
    };
    let row = golden
        .rows
        .iter()
        .filter(|row| row.place.ends_with(label))
        .find(runs)
        .ok_or_else(|| format!("no row {label} of {file} runs {script}"))?;
    let scripts = golden
        .suites
        .get(&row.suite)
        .ok_or_else(|| format!("unknown suite {}", row.suite))?;
    let mut cells = row.cells.iter();
    for name in scripts {
        let steps = golden
            .scripts
            .get(name)
            .ok_or_else(|| format!("unknown script {name}"))?;
        if name != script {
            for _step in steps {
                let _cell = cells.next();
            }
            continue;
        }
        let request: openrtb2::BidRequest =
            unmarshal(&row.doc).map_err(|e| format!("{label}: {e}"))?;
        let mut runner = Runner::new(request);
        let mut out = Vec::new();
        for step in steps {
            let cell = cells.next().cloned().unwrap_or(Cell::Skipped);
            out.push((step.clone(), cell, runner.step(step)?));
        }
        return Ok(out);
    }
    Err(format!("{label} runs no script {script}"))
}

/// Whether Go's observation of a step starts with an error.
fn fails_in_go(cell: &Cell) -> bool {
    let Cell::Observed(parts) = cell else {
        return false;
    };
    parts.first().is_some_and(|part| !part.error().is_empty())
}

/// The error texts of Prebid's `UnmarshalJSON` methods come out of the wrapper's decodes as Go
/// writes them, without the position `serde_json` puts after them
/// (`json.prebid-message-context`, restored): `get:device` on a device ext whose interstitial
/// width is a string fails with Go's text, byte for byte.
#[test]
fn prebid_messages_are_bare() -> TestResult {
    const WIDTH: &str =
        "request.device.ext.prebid.interstitial.minwidthperc must be a number between 0 and 100";
    let steps = script_steps("review.tsv.gz", "review device ext 0", "get")?;
    let (_step, go, port) = steps
        .iter()
        .find(|(_, go, _)| fails_in_go(go))
        .ok_or("no step of the script fails in Go")?;
    let (Cell::Observed(go), Outcome::Observed(port)) = (go, port) else {
        return Err(format!("port {port:?}, Go {go:?}"));
    };
    let go_error = go.first().map(Part::error);
    ensure_eq(&go_error, &Some(WIDTH), "Go's error")?;
    ensure_eq(
        &port.first().map(Part::error),
        &go_error,
        "the port's error",
    )
}

/// An ext member `SetExt` is given that is not JSON: Go's map holds the bytes and its rebuild
/// writes nothing in their place, the port's map takes only JSON (`json.raw-not-json`). The
/// mutations' `setrawext` scripts give `malformed`, which a `RawValue` cannot hold; the review's
/// `special badraw` sets an EID ext of `{bad`, which Go's rebuild drops and the port's fails to
/// write.
#[test]
fn raw_members_must_be_json() -> TestResult {
    let steps = script_steps(
        "review.tsv.gz",
        "review user ext 0",
        "review special badraw",
    )?;
    let rebuild = steps
        .get(1)
        .ok_or("the badraw script has no rebuild step")?;
    let (step, Cell::Observed(go), Outcome::Observed(port)) = rebuild else {
        return Err(format!("{rebuild:?}"));
    };
    ensure_eq(&step.as_str(), &"rebuild", "the step")?;
    ensure_eq(
        &go.first().map(Part::error),
        &Some(""),
        "Go's rebuild error",
    )?;
    if port.first().map_or_default(Part::error).is_empty() {
        return Err(format!(
            "the port's rebuild wrote a raw member that is not JSON: {port:?}"
        ));
    }
    ensure_eq(
        &RawValue::from_string("malformed".to_owned()).is_err(),
        &true,
        "a RawValue of malformed",
    )
}

/// The first JSON part and the first error of an observation.
fn json_and_error(parts: &[Part]) -> (Option<&str>, Option<&str>) {
    let json = parts.iter().find_map(|part| match part {
        Part::Json(json) => Some(json.as_str()),
        Part::Err(_) | Part::MarshalErr(_) | Part::Text(_) | Part::Map(_) => None,
    });
    let error = parts
        .iter()
        .find(|part| matches!(part, Part::Err(_)))
        .map(Part::error);
    (json, error)
}

/// An ext map with a null member, rebuilt (`json.ext-null-member`): once `imp.ext.tid` is set,
/// Go's rebuild of `{"prebid":null,"other":[1,"x"]}` writes the nil member as nothing, so the
/// ext's bytes are not JSON, the request is written without a value for the ext, and reading
/// the ext again fails; the port writes the member `null` and reads the ext back.
#[test]
fn ext_null_members_written_null() -> TestResult {
    let steps = script_steps("mutations.tsv.gz", "imp ext prebid 3", "imps")?;
    let observed = |name: &str| {
        steps
            .iter()
            .find(|(step, ..)| step == name)
            .ok_or_else(|| format!("the imps script has no {name} step"))
            .and_then(|(_, go, port)| match (go, port) {
                (Cell::Observed(go), Outcome::Observed(port)) => Ok((go, port)),
                _ => Err(format!("{name}: port {port:?}, Go {go:?}")),
            })
    };
    let (go_rebuild, port_rebuild) = observed("rebuild")?;
    ensure_eq(
        &json_and_error(go_rebuild).0,
        &Some(r#"{"id":"r","imp":[{"id":"1","ext":},{"id":"2","ext":{"tid":"tid"}}]}"#),
        "Go's rebuilt request",
    )?;
    ensure_eq(
        &json_and_error(port_rebuild).0,
        &Some(concat!(
            r#"{"id":"r","imp":[{"id":"1","ext":{"other":[1,"x"],"prebid":null,"tid":"tid"}},"#,
            r#"{"id":"2","ext":{"tid":"tid"}}]}"#
        )),
        "the port's rebuilt request",
    )?;
    let (go, port) = observed("get:imp")?;
    ensure_eq(
        &json_and_error(go).1,
        &Some("do not know how to skip: 44"),
        "Go's error reading the rebuilt ext",
    )?;
    ensure_eq(
        &json_and_error(port).1,
        &Some(""),
        "the port's error reading the rebuilt ext",
    )
}
