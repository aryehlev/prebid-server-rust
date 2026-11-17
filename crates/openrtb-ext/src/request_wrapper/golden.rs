//! Replays, byte for byte, the differential goldens that `testdata/go/request_wrapper` writes
//! with Go's `RequestWrapper`: every bid request of prebid-server's test corpora, generated
//! mutations and the review's requests, run through scripts of wrapper operations (getters,
//! setters, rebuilds, the conversions, the `Clear*` functions, the impression wrappers, the clones
//! and the review's sequences, such as exchange/utils.go's per-bidder request), with the
//! observation of every step.
//!
//! An observation is a sequence of parts, each its decimal length, `:` and its bytes. Regenerate
//! the goldens from the repository root with
//! `GOTOOLCHAIN=go1.26.2 go run ./rust/crates/openrtb-ext/testdata/go/request_wrapper`.
//!
//! One thing is not compared byte for byte: an error text that is not valid UTF-8. jsoniter
//! quotes the byte it did not expect as it is (`unexpected character: \xff`), and a [`GoError`]
//! holds its message as a Rust string, with U+FFFD in place of each invalid byte. Go records such
//! an observation as `~`, its hash and the hash of that lossy form, which the replay compares.

use core::str::from_utf8;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use pbs_errortypes::GoError;
use pbs_gocompat::strconv::unquote_bytes;
use pbs_gojson::{GoEncode, JsoniterDecode, RawJson, jsonutil};
use pbs_openrtb::openrtb2;
use serde_json::Value;

use super::{ImpWrapper, ImpWrapperMut, RequestWrapper};
use crate::{
    ConsentedProvidersSettingsIn, ConsentedProvidersSettingsOut, ExtAppPrebid, ExtDevicePrebid,
    ExtImpPrebid, ExtImpPrebidFloors, ExtRegsDSA, ExtRequestPrebid, ExtUserPrebid, clear26_fields,
    clear202211_fields, clear202303_fields, clear202309_fields, clear202402_fields,
    clear202409_fields, convert_down_to25, convert_up_to26,
};

/// A test result whose error is the failure message.
type TestResult<T = ()> = Result<T, String>;

/// The text of Go's panic on a write into a nil map, which the Rust wrapper returns as an error.
const NIL_MAP_PANIC: &str = "assignment to entry in nil map";

/// The crate directory.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The repository root: the Go module whose corpora the goldens were made from.
fn repo_root() -> PathBuf {
    crate_dir().join("../../..")
}

/// The file's bytes.
fn read(path: &Path) -> TestResult<Vec<u8>> {
    fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// FNV-1a 64 (Go's `hash/fnv` `New64a`).
fn fnv64(data: &[u8]) -> u64 {
    data.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// Text for a failure message: the bytes, lossily, cut at 400 bytes.
fn show(bytes: &[u8]) -> String {
    let cut = bytes.get(..400).unwrap_or(bytes);
    let mut s = String::from_utf8_lossy(cut).into_owned();
    if cut.len() < bytes.len() {
        s.push_str("...");
    }
    s
}

/// The message for a corpus that changed after the goldens were written.
fn regenerate(what: &str) -> String {
    format!(
        "{what} after the goldens were written; regenerate them with \
         `GOTOOLCHAIN=go1.26.2 go run ./rust/crates/openrtb-ext/testdata/go/request_wrapper`"
    )
}

/// A golden file: its corpus, scripts and suites, and its request rows.
#[derive(Debug, Default)]
struct Golden {
    /// The directories of the `#root` rows.
    roots: Vec<String>,
    /// The path and bytes of each `#file` row.
    files: Vec<(String, Vec<u8>)>,
    /// The steps of each script.
    scripts: BTreeMap<String, Vec<String>>,
    /// The scripts of each suite.
    suites: BTreeMap<String, Vec<String>>,
    /// The request rows: the fields after the `#` rows.
    rows: Vec<Vec<Vec<u8>>>,
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

/// Reads a golden file of `testdata/go/request_wrapper`, checking its corpus files against the
/// hashes Go recorded.
fn golden(file: &str) -> TestResult<Golden> {
    let path = crate_dir().join("testdata/go/request_wrapper").join(file);
    let data = read(&path)?;
    let content = from_utf8(&data).map_err(|e| format!("{}: not ASCII: {e}", path.display()))?;
    let mut golden = Golden::default();
    for (n, line) in content.lines().enumerate() {
        let row = line
            .split('\t')
            .map(|field| unquote_bytes(field).map_err(|e| format!("{file}:{}: {e}", n + 1)))
            .collect::<TestResult<Vec<Vec<u8>>>>()?;
        let second = row.get(1).map(|field| text(field)).transpose()?;
        match (row.first().map(Vec::as_slice), second) {
            (Some(b"#go"), _) => {}
            (Some(b"#root"), Some(root)) => golden.roots.push(root),
            (Some(b"#file"), Some(corpus_path)) => {
                let corpus = read(&repo_root().join(&corpus_path))?;
                let recorded = row.get(2).map_or(&[][..], Vec::as_slice);
                if hash(&corpus).as_bytes() != recorded {
                    return Err(regenerate(&format!("{corpus_path} changed")));
                }
                golden.files.push((corpus_path, corpus));
            }
            (Some(b"#script"), Some(name)) => {
                let _previous = golden.scripts.insert(name, names(&row)?);
            }
            (Some(b"#suite"), Some(name)) => {
                let _previous = golden.suites.insert(name, names(&row)?);
            }
            _ => golden.rows.push(row),
        }
    }
    Ok(golden)
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

/// What the replay of a golden file found.
#[derive(Debug, Default)]
struct Tally {
    /// The request rows replayed.
    rows: usize,
    /// The steps whose observation was compared.
    steps: usize,
    /// The steps where Go panicked on a write into a nil map, and Rust returned that error.
    nil_map_panics: usize,
    /// The steps whose observation has an error text that is not valid UTF-8 in Go, compared in
    /// the lossy form.
    lossy_errors: usize,
    /// Every difference.
    failures: Vec<String>,
}

/// The FNV-1a 64 of an observation as Go writes it: 16 hex digits.
fn hash(obs: &[u8]) -> String {
    format!("{:016x}", fnv64(obs))
}

/// Whether a cell matches the observation of a step (`Ok` with the bytes) or its panic (`Err`
/// with Go's panic text), given the earlier observations of the row and whether they matched.
fn matches(
    cell: &[u8],
    got: &Result<Vec<u8>, String>,
    earlier: &[(Option<Vec<u8>>, bool)],
) -> bool {
    match (cell.split_first(), got) {
        (Some((b'!', panic)), Err(got_panic)) => panic == got_panic.as_bytes(),
        (Some((b'\'', want)), Ok(obs)) => want == obs.as_slice(),
        (Some((b'#', want)), Ok(obs)) => hash(obs).as_bytes() == want,
        (Some((b'~', hashes)), Ok(obs)) => hashes
            .split(|&b| b == b':')
            .nth(1)
            .is_some_and(|lossy| hash(obs).as_bytes() == lossy),
        (Some((b'=', index)), Ok(obs)) => from_utf8(index)
            .ok()
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| earlier.get(index.checked_sub(1)?))
            .is_some_and(|(earlier_obs, matched)| {
                *matched && earlier_obs.as_deref() == Some(obs.as_slice())
            }),
        _ => false,
    }
}

/// Replays the request rows of a golden file.
fn replay(golden: &Golden, file: &str) -> TestResult<Tally> {
    let mut tally = Tally::default();
    let mut previous: Option<(Vec<u8>, String)> = None;
    for (n, row) in golden.rows.iter().enumerate() {
        let [doc_ref, label, suite, cells @ ..] = row.as_slice() else {
            return Err(format!("{file}: row {n}: too few fields"));
        };
        let (doc, place) = if doc_ref.as_slice() == b"^" {
            previous
                .clone()
                .ok_or_else(|| format!("{file}: row {n}: ^ without a row before"))?
        } else {
            let (doc, path) = document(doc_ref, &golden.files)?;
            let place = format!("{path} {}", String::from_utf8_lossy(label));
            previous = Some((doc.clone(), place.clone()));
            (doc, place)
        };
        let suite = text(suite)?;
        let scripts = golden
            .suites
            .get(&suite)
            .ok_or_else(|| format!("{file}: row {n}: unknown suite {suite}"))?;
        let mut request = openrtb2::BidRequest::default();
        if let Err(e) = jsonutil::unmarshal(&doc, &mut request) {
            tally
                .failures
                .push(format!("{place}: decoding the request: {e}"));
            continue;
        }
        tally.rows += 1;
        let mut cells = cells.iter();
        let mut earlier: Vec<(Option<Vec<u8>>, bool)> = Vec::new();
        for script in scripts {
            let steps = golden
                .scripts
                .get(script)
                .ok_or_else(|| format!("{file}: unknown script {script}"))?;
            let mut runner = Runner {
                w: RequestWrapper::new(request.clone()),
            };
            let mut stopped = false;
            for (i, step) in steps.iter().enumerate() {
                let cell = cells.next().map_or(&[][..], Vec::as_slice);
                if stopped {
                    if cell != b"-" {
                        tally.failures.push(format!(
                            "{place}: script {script} step {i} `{step}`: Rust stopped, Go did not"
                        ));
                    }
                    earlier.push((None, false));
                    continue;
                }
                let got = runner.step(step)?;
                tally.steps += 1;
                let matched = matches(cell, &got, &earlier);
                if !matched {
                    let shown = match &got {
                        Ok(obs) => show(obs),
                        Err(panic) => format!("panic {panic}"),
                    };
                    tally.failures.push(format!(
                        "{place}: script {script} step {i} `{step}`: got {shown}; want {}",
                        show(cell)
                    ));
                }
                if matched && cell.first() == Some(&b'~') {
                    tally.lossy_errors += 1;
                }
                if got.is_err() {
                    stopped = true;
                    if matched {
                        tally.nil_map_panics += 1;
                    }
                }
                earlier.push((got.ok(), matched));
            }
        }
        if cells.next().is_some() {
            tally
                .failures
                .push(format!("{place}: more cells than steps"));
        }
    }
    Ok(tally)
}

/// Fails with the first failures of a replay, and when it replayed fewer than `min_rows` rows.
fn report(file: &str, tally: &Tally, min_rows: usize) -> TestResult {
    if !tally.failures.is_empty() {
        let shown: Vec<&str> = tally.failures.iter().take(20).map(String::as_str).collect();
        return Err(format!(
            "{file}: {} failures in {} steps; the first {}:\n{}",
            tally.failures.len(),
            tally.steps,
            shown.len(),
            shown.join("\n")
        ));
    }
    if tally.rows < min_rows {
        return Err(format!("{file}: only {} requests", tally.rows));
    }
    Ok(())
}

/// The JSON files below `dir` (repository-relative, `/`-separated), appended to `out`: as the
/// generator's `filepath.WalkDir`, files whose name ends in `.json`, not following symbolic
/// links to directories.
fn json_files(dir: &str, out: &mut Vec<String>) -> TestResult {
    let path = repo_root().join(dir);
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

/// The fixture roots of `rust/testdata/go_fixture_manifest.json` as the generator reads them:
/// distinct, without missing or empty ones.
fn fixture_roots() -> TestResult<BTreeSet<String>> {
    let path = repo_root().join("rust/testdata/go_fixture_manifest.json");
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
    let fixture_roots = fixture_roots()?;
    let missing: Vec<&String> = fixture_roots.difference(&roots).collect();
    if !missing.is_empty() {
        return Err(regenerate(&format!("fixture roots {missing:?} were added")));
    }
    let mut on_disk = Vec::new();
    for root in &golden.roots {
        json_files(root, &mut on_disk)?;
    }
    let on_disk: BTreeSet<String> = on_disk.into_iter().collect();
    let recorded: BTreeSet<String> = golden.files.iter().map(|(path, _)| path.clone()).collect();
    let added: Vec<&String> = on_disk.difference(&recorded).collect();
    let removed: Vec<&String> = recorded.difference(&on_disk).collect();
    if !added.is_empty() || !removed.is_empty() {
        return Err(regenerate(&format!(
            "corpus files {added:?} were added and {removed:?} removed"
        )));
    }
    Ok(())
}

/// Every bid request of the adapter, exchange, endpoint and first-party data test corpora. Two
/// requests of `exchange/exchangetest/request-imp-ext-prebid-filtering.json` (whose
/// `is_rewarded_inventory` is `true`, not a number) give an error with an invalid UTF-8 byte in
/// 6 steps, compared in the lossy form.
#[test]
fn corpus() -> TestResult {
    let golden = golden("corpus.tsv")?;
    check_corpus(&golden)?;
    let tally = replay(&golden, "corpus.tsv")?;
    report("corpus.tsv", &tally, 4900)?;
    if tally.nil_map_panics != 0 {
        return Err(format!(
            "{} nil map panics in the corpus",
            tally.nil_map_panics
        ));
    }
    if tally.lossy_errors != 6 {
        return Err(format!("{} lossy error texts, want 6", tally.lossy_errors));
    }
    Ok(())
}

/// The generated requests, and the setter scripts. Go panics on a write into a nil map (a
/// member set after `SetExt(nil)`, or into an ext that is the JSON `null`) in 72 steps, where
/// the Rust wrapper returns the panic's text as an error.
#[test]
fn mutations() -> TestResult {
    let golden = golden("mutations.tsv")?;
    let tally = replay(&golden, "mutations.tsv")?;
    report("mutations.tsv", &tally, 220)?;
    if tally.nil_map_panics != 72 {
        return Err(format!("{} nil map panics, want 72", tally.nil_map_panics));
    }
    Ok(())
}

/// The review suites: the review requests through the corpus and review scripts, and every
/// corpus request through the review corpus scripts (see `testdata/go/request_wrapper/review.go`).
#[test]
fn review() -> TestResult {
    let golden = golden("review.tsv")?;
    check_corpus(&golden)?;
    let tally = replay(&golden, "review.tsv")?;
    report("review.tsv", &tally, 5200)
}

/// An observation: a sequence of parts, each its decimal length, `:` and its bytes.
#[derive(Debug, Default)]
struct Obs(Vec<u8>);

impl Obs {
    /// Appends a part.
    fn part(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes.len().to_string().as_bytes());
        self.0.push(b':');
        self.0.extend_from_slice(bytes);
    }

    /// Appends a text part.
    fn text(&mut self, s: &str) {
        self.part(s.as_bytes());
    }

    /// Appends an error's text (empty without one).
    fn err(&mut self, err: Option<&GoError>) {
        self.text(err.map_or("", GoError::message));
    }

    /// Appends a value as `jsonutil.Marshal` writes it (`!` and the error when it fails).
    fn marshal<T: GoEncode + ?Sized>(&mut self, v: &T) {
        match jsonutil::marshal(v) {
            Ok(json) => self.part(&json),
            Err(e) => self.text(&format!("!{e}")),
        }
    }

    /// Appends a step's error (empty without one) and the request.
    fn request(&mut self, err: Option<&GoError>, w: &RequestWrapper) {
        self.err(err);
        self.marshal(&w.bid_request());
    }

    /// Appends a flag: `t` or `f`.
    fn bool(&mut self, b: bool) {
        self.text(if b { "t" } else { "f" });
    }

    /// Appends a number.
    fn int(&mut self, n: usize) {
        self.text(&n.to_string());
    }

    /// Appends an ext map: its members in key order, each the key as `jsonutil.Marshal` writes
    /// it and the value's bytes verbatim (`<nil>` for a nil value).
    fn raw_map(&mut self, map: &BTreeMap<String, RawJson>) {
        let mut inner = Self::default();
        for (key, value) in map {
            inner.marshal(key);
            if value.is_nil() {
                inner.text("<nil>");
            } else {
                inner.part(value.as_bytes());
            }
        }
        self.part(&inner.0);
    }
}

/// A step's argument decoded with `jsonutil.Unmarshal`.
fn decode<T: JsoniterDecode + Default>(arg: &str) -> TestResult<T> {
    let mut v = T::default();
    jsonutil::unmarshal(arg.as_bytes(), &mut v).map_err(|e| format!("step argument {arg}: {e}"))?;
    Ok(v)
}

/// The objects of the `all` steps, in order.
const OBJECTS: [&str; 9] = [
    "request", "user", "device", "app", "regs", "site", "dooh", "source", "imp",
];

/// The objects of a step's target: all of them for `all`.
fn targets(target: &str) -> Vec<&str> {
    if target == "all" {
        OBJECTS.to_vec()
    } else {
        vec![target]
    }
}

/// The result of a rebuild as the replay sees it: Go's panic text for the nil map error, which
/// Go panics with.
fn panic_of(result: &Result<(), GoError>) -> Option<String> {
    result
        .as_ref()
        .err()
        .filter(|e| e.message() == NIL_MAP_PANIC)
        .map(|e| e.message().to_owned())
}

/// Runs the steps of a script on one wrapper, as the Go generator's runner does.
#[derive(Debug)]
struct Runner {
    /// The wrapper.
    w: RequestWrapper,
}

impl Runner {
    /// Runs a step: its observation, or (`Err` inside) Go's panic text.
    fn step(&mut self, step: &str) -> TestResult<Result<Vec<u8>, String>> {
        let mut o = Obs::default();
        let (name, arg) = step.split_once('=').unwrap_or((step, ""));
        let (op, target) = name.split_once(':').unwrap_or((name, ""));
        let panic = match op {
            "get" => {
                self.get(target, &mut o)?;
                None
            }
            "set" => {
                self.set(target, arg, &mut o)?;
                None
            }
            "special" => {
                self.special(target, &mut o)?;
                None
            }
            "setext" => {
                let map: Option<BTreeMap<String, RawJson>> = decode(arg)?;
                self.set_ext(target, Some(&map), &mut o)?;
                None
            }
            "setrawext" => {
                let (key, raw) = arg.split_once('=').unwrap_or((arg, ""));
                let map = BTreeMap::from([(key.to_owned(), RawJson::from(raw))]);
                self.set_ext(target, Some(&Some(map)), &mut o)?;
                None
            }
            "touchext" => {
                for obj in targets(target) {
                    self.set_ext(obj, None, &mut o)?;
                }
                None
            }
            "setimpraw" => {
                for mut iw in self.w.get_imp() {
                    if let Some(imp) = iw.imp_mut() {
                        imp.ext = RawJson::from(arg);
                    }
                }
                self.w.set_imp_from_get_imp();
                o.request(None, &self.w);
                None
            }
            "touch" => {
                for obj in targets(target) {
                    self.touch(obj, &mut o)?;
                }
                None
            }
            _ => self.request_step(op, &mut o)?,
        };
        Ok(panic.map_or(Ok(o.0), Err))
    }

    /// Runs a step on the request (or, through [`Runner::imp_step`], the impression wrappers);
    /// `Some` is Go's panic text.
    fn request_step(&mut self, op: &str, o: &mut Obs) -> TestResult<Option<String>> {
        let w = &mut self.w;
        match op {
            "snap" => o.request(None, w),
            "rebuild" => {
                let result = w.rebuild_request();
                if let Some(panic) = panic_of(&result) {
                    return Ok(Some(panic));
                }
                o.request(result.err().as_ref(), w);
            }
            "up" => {
                let result = convert_up_to26(w);
                o.request(result.err().as_ref(), w);
            }
            "down" => {
                let result = convert_down_to25(w);
                o.request(result.err().as_ref(), w);
            }
            "clear26" => clear(w, clear26_fields, o),
            "clear202211" => clear(w, clear202211_fields, o),
            "clear202303" => clear(w, clear202303_fields, o),
            "clear202309" => clear(w, clear202309_fields, o),
            "clear202402" => clear(w, clear202402_fields, o),
            "clear202409" => clear(w, clear202409_fields, o),
            "clone" => {
                *w = w.clone();
                o.int(w.len_imp());
            }
            "cloneclear" => {
                *w = w.clone_and_clear_imp_wrappers();
                o.int(w.len_imp());
            }
            "cloneclearkeep" => {
                let _clone = w.clone_and_clear_imp_wrappers();
                o.int(w.len_imp());
            }
            "exchangecopy" => {
                let bid_request = w.bid_request().cloned().map(Box::new);
                *w = w.clone_and_clear_imp_wrappers_with(bid_request);
                o.int(w.len_imp());
            }
            "partialclone" => {
                let bid_request = w.bid_request().cloned().map(Box::new);
                w.set_bid_request(bid_request);
                o.request(None, w);
            }
            "nilreq" => {
                w.set_bid_request(None);
                o.request(None, w);
            }
            _ => return self.imp_step(op, o),
        }
        Ok(None)
    }

    /// Runs a step on the impressions or their wrappers; `Some` is Go's panic text.
    fn imp_step(&mut self, op: &str, o: &mut Obs) -> TestResult<Option<String>> {
        let w = &mut self.w;
        match op {
            "setimprev" => {
                let rev = w
                    .get_imp()
                    .iter()
                    .rev()
                    .map(ImpWrapperMut::to_imp_wrapper)
                    .collect();
                w.set_imp(Some(rev));
                o.request(None, w);
            }
            "setimpdrop" => {
                let imps: Vec<ImpWrapper> = w
                    .get_imp()
                    .iter()
                    .map(ImpWrapperMut::to_imp_wrapper)
                    .collect();
                if !imps.is_empty() {
                    w.set_imp(Some(imps.into_iter().skip(1).collect()));
                }
                o.request(None, w);
            }
            "setimpnil" => {
                w.set_imp(None);
                o.request(None, w);
            }
            "assignlast" => {
                let last = w
                    .bid_request()
                    .and_then(|req| req.imp.as_ref()?.last().cloned());
                w.assign_imp(last.map(|imp| vec![imp]));
                o.request(None, w);
            }
            "lenimp" => o.int(w.len_imp()),
            "getimp" => {
                let imps = w.get_imp();
                o.int(imps.len());
                for iw in &imps {
                    o.marshal(&iw.imp());
                }
            }
            "setimp" => {
                w.set_imp_from_get_imp();
                o.request(None, w);
            }
            "assignrev" => {
                if let Some(imps) = w.bid_request().and_then(|req| req.imp.clone()) {
                    w.assign_imp(Some(imps.into_iter().rev().collect()));
                }
                o.request(None, w);
            }
            "dropimp" => {
                w.assign_imp(None);
                o.request(None, w);
            }
            "rebuildimp" => {
                for mut iw in w.get_imp() {
                    let result = iw.rebuild_imp();
                    if let Some(panic) = panic_of(&result) {
                        return Ok(Some(panic));
                    }
                    o.err(result.err().as_ref());
                    o.marshal(&iw.imp());
                }
            }
            "getorcreate" => {
                for mut iw in w.get_imp() {
                    let (e, _err) = iw.get_imp_ext_pair();
                    o.marshal(e.get_or_create_prebid());
                    o.bool(e.dirty());
                }
            }
            _ => return Err(format!("unknown step {op}")),
        }
        Ok(None)
    }

    /// Records an object's `GetXExt` error and every getter.
    fn get(&mut self, obj: &str, o: &mut Obs) -> TestResult {
        let w = &mut self.w;
        match obj {
            "request" => {
                let (e, err) = w.get_request_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_prebid());
                o.marshal(&e.get_s_chain());
                o.bool(e.dirty());
            }
            "user" => {
                let (e, err) = w.get_user_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_consent());
                o.marshal(&e.get_prebid());
                o.marshal(&e.get_eid());
                o.marshal(&e.get_consented_providers_settings_in());
                o.marshal(&e.get_consented_providers_settings_out());
                o.bool(e.dirty());
            }
            "device" => {
                let (e, err) = w.get_device_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_prebid());
                o.marshal(e.get_c_dep());
                o.bool(e.dirty());
            }
            "app" => {
                let (e, err) = w.get_app_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_prebid());
                o.bool(e.dirty());
            }
            "regs" => {
                let (e, err) = w.get_reg_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_dsa());
                o.marshal(&e.get_gdpr());
                o.marshal(&e.get_gpc());
                o.marshal(e.get_us_privacy());
                o.bool(e.dirty());
            }
            "site" => {
                let (e, err) = w.get_site_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_amp());
                o.bool(e.dirty());
            }
            "dooh" => {
                let (e, err) = w.get_dooh_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.bool(e.dirty());
            }
            "source" => {
                let (e, err) = w.get_source_ext_pair();
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_s_chain());
                o.bool(e.dirty());
            }
            "imp" => {
                let mut imps = w.get_imp();
                o.int(imps.len());
                for iw in &mut imps {
                    let (e, err) = iw.get_imp_ext_pair();
                    o.err(err.as_ref());
                    o.raw_map(&e.get_ext());
                    o.marshal(&e.get_prebid());
                    o.marshal(&e.get_data());
                    o.marshal(e.get_tid());
                    o.marshal(e.get_gp_id());
                    o.bool(e.dirty());
                }
            }
            _ => return Err(format!("unknown object {obj}")),
        }
        Ok(())
    }

    /// Calls a member's setter with `arg` decoded into its parameter.
    fn set(&mut self, target: &str, arg: &str, o: &mut Obs) -> TestResult {
        let w = &mut self.w;
        match target {
            "request.prebid" => {
                let e = w.get_request_ext_pair().0;
                e.set_prebid(decode::<Option<ExtRequestPrebid>>(arg)?);
                o.bool(e.dirty());
            }
            "request.schain" => {
                let e = w.get_request_ext_pair().0;
                e.set_s_chain(decode::<Option<openrtb2::SupplyChain>>(arg)?);
                o.bool(e.dirty());
            }
            "user.consent" => {
                let e = w.get_user_ext_pair().0;
                e.set_consent(decode::<Option<String>>(arg)?);
                o.bool(e.dirty());
            }
            "user.prebid" => {
                let e = w.get_user_ext_pair().0;
                e.set_prebid(decode::<Option<ExtUserPrebid>>(arg)?);
                o.bool(e.dirty());
            }
            "user.eids" => {
                let eids = if arg == "&nil" {
                    Some(None)
                } else {
                    decode::<Option<Option<Vec<openrtb2::EID>>>>(arg)?
                };
                let e = w.get_user_ext_pair().0;
                e.set_eid(eids);
                o.bool(e.dirty());
            }
            "user.cpsin" => {
                let e = w.get_user_ext_pair().0;
                e.set_consented_providers_settings_in(decode::<
                    Option<ConsentedProvidersSettingsIn>,
                >(arg)?);
                o.bool(e.dirty());
            }
            "user.cpsout" => {
                let e = w.get_user_ext_pair().0;
                e.set_consented_providers_settings_out(decode::<
                    Option<ConsentedProvidersSettingsOut>,
                >(arg)?);
                o.bool(e.dirty());
            }
            _ => return self.set_other(target, arg, o),
        }
        Ok(())
    }

    /// [`Runner::set`] for the members of the objects but the request and the user.
    fn set_other(&mut self, target: &str, arg: &str, o: &mut Obs) -> TestResult {
        let w = &mut self.w;
        match target {
            "device.prebid" => {
                let e = w.get_device_ext_pair().0;
                e.set_prebid(decode::<Option<ExtDevicePrebid>>(arg)?);
                o.bool(e.dirty());
            }
            "device.cdep" => {
                let e = w.get_device_ext_pair().0;
                e.set_c_dep(decode::<String>(arg)?);
                o.bool(e.dirty());
            }
            "app.prebid" => {
                let e = w.get_app_ext_pair().0;
                e.set_prebid(decode::<Option<ExtAppPrebid>>(arg)?);
                o.bool(e.dirty());
            }
            "regs.dsa" => {
                let e = w.get_reg_ext_pair().0;
                e.set_dsa(decode::<Option<ExtRegsDSA>>(arg)?);
                o.bool(e.dirty());
            }
            "regs.gdpr" => {
                let e = w.get_reg_ext_pair().0;
                e.set_gdpr(decode::<Option<i8>>(arg)?);
                o.bool(e.dirty());
            }
            "regs.gpc" => {
                let e = w.get_reg_ext_pair().0;
                e.set_gpc(decode::<Option<String>>(arg)?);
                o.bool(e.dirty());
            }
            "regs.usprivacy" => {
                let e = w.get_reg_ext_pair().0;
                e.set_us_privacy(decode::<String>(arg)?);
                o.bool(e.dirty());
            }
            "site.amp" => {
                let e = w.get_site_ext_pair().0;
                e.set_amp(decode::<Option<i8>>(arg)?);
                o.bool(e.dirty());
            }
            "source.schain" => {
                let e = w.get_source_ext_pair().0;
                e.set_s_chain(decode::<Option<openrtb2::SupplyChain>>(arg)?);
                o.bool(e.dirty());
            }
            "imp.prebid" => {
                for mut iw in w.get_imp() {
                    let e = iw.get_imp_ext_pair().0;
                    e.set_prebid(decode::<Option<ExtImpPrebid>>(arg)?);
                    o.bool(e.dirty());
                }
            }
            "imp.tid" => {
                for mut iw in w.get_imp() {
                    let e = iw.get_imp_ext_pair().0;
                    e.set_tid(decode::<String>(arg)?);
                    o.bool(e.dirty());
                }
            }
            _ => return Err(format!("unknown member {target}")),
        }
        Ok(())
    }

    /// Calls a setter with a value that JSON cannot give, as the Go generator's `special` does:
    /// NaN and infinite floats, and raw messages that are invalid or not compact.
    fn special(&mut self, target: &str, o: &mut Obs) -> TestResult {
        let w = &mut self.w;
        match target {
            "request.prebid.nan" => {
                let e = w.get_request_ext_pair().0;
                e.set_prebid(Some(ExtRequestPrebid {
                    debug: true,
                    bid_adjustment_factors: Some(BTreeMap::from([("a".to_owned(), f64::NAN)])),
                    ..ExtRequestPrebid::default()
                }));
                o.bool(e.dirty());
            }
            "imp.prebid.inf" => {
                for mut iw in w.get_imp() {
                    let e = iw.get_imp_ext_pair().0;
                    e.set_prebid(Some(ExtImpPrebid {
                        floors: Some(Box::new(ExtImpPrebidFloors {
                            floor_rule_value: f64::INFINITY,
                            ..ExtImpPrebidFloors::default()
                        })),
                        ..ExtImpPrebid::default()
                    }));
                    o.bool(e.dirty());
                }
            }
            "user.eids.badraw" => {
                let e = w.get_user_ext_pair().0;
                e.set_eid(Some(Some(vec![openrtb2::EID {
                    source: "s".to_owned(),
                    ext: RawJson::from("{bad"),
                    ..openrtb2::EID::default()
                }])));
                o.bool(e.dirty());
            }
            "source.schain.spacedraw" => {
                let e = w.get_source_ext_pair().0;
                e.set_s_chain(Some(openrtb2::SupplyChain {
                    ver: "1".to_owned(),
                    ext: RawJson::from(r#" { "a" : 1 } "#),
                    ..openrtb2::SupplyChain::default()
                }));
                o.bool(e.dirty());
            }
            _ => return Err(format!("unknown special member {target}")),
        }
        Ok(())
    }

    /// Calls each object's `SetExt` with `map`, or with its `GetExt()` when `map` is `None`.
    fn set_ext(
        &mut self,
        obj: &str,
        map: Option<&Option<BTreeMap<String, RawJson>>>,
        o: &mut Obs,
    ) -> TestResult {
        let w = &mut self.w;
        macro_rules! set_ext_of {
            ($e:expr) => {{
                let e = $e;
                let ext = map.cloned().unwrap_or_else(|| Some(e.get_ext()));
                e.set_ext(ext);
                o.bool(e.dirty());
            }};
        }
        match obj {
            "request" => set_ext_of!(w.get_request_ext_pair().0),
            "user" => set_ext_of!(w.get_user_ext_pair().0),
            "device" => set_ext_of!(w.get_device_ext_pair().0),
            "app" => set_ext_of!(w.get_app_ext_pair().0),
            "regs" => set_ext_of!(w.get_reg_ext_pair().0),
            "site" => set_ext_of!(w.get_site_ext_pair().0),
            "dooh" => set_ext_of!(w.get_dooh_ext_pair().0),
            "source" => set_ext_of!(w.get_source_ext_pair().0),
            "imp" => {
                for mut iw in w.get_imp() {
                    set_ext_of!(iw.get_imp_ext_pair().0);
                }
            }
            _ => return Err(format!("unknown object {obj}")),
        }
        Ok(())
    }

    /// Sets every member of an object to what its getter returns.
    fn touch(&mut self, obj: &str, o: &mut Obs) -> TestResult {
        let w = &mut self.w;
        match obj {
            "request" => {
                let e = w.get_request_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                e.set_prebid(e.get_prebid().cloned());
                e.set_s_chain(e.get_s_chain().cloned());
                o.bool(e.dirty());
            }
            "user" => {
                let e = w.get_user_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                e.set_consent(e.get_consent().map(str::to_owned));
                e.set_prebid(e.get_prebid().cloned());
                e.set_eid(e.get_eid().map(|eids| eids.map(<[openrtb2::EID]>::to_vec)));
                e.set_consented_providers_settings_in(
                    e.get_consented_providers_settings_in().cloned(),
                );
                e.set_consented_providers_settings_out(
                    e.get_consented_providers_settings_out().cloned(),
                );
                o.bool(e.dirty());
            }
            "device" => {
                let e = w.get_device_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                e.set_prebid(e.get_prebid().cloned());
                e.set_c_dep(e.get_c_dep().to_owned());
                o.bool(e.dirty());
            }
            "app" => {
                let e = w.get_app_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                e.set_prebid(e.get_prebid().cloned());
                o.bool(e.dirty());
            }
            "regs" => {
                let e = w.get_reg_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                e.set_dsa(e.get_dsa().cloned());
                e.set_gdpr(e.get_gdpr());
                e.set_gpc(e.get_gpc().map(str::to_owned));
                e.set_us_privacy(e.get_us_privacy().to_owned());
                o.bool(e.dirty());
            }
            _ => return self.touch_other(obj, o),
        }
        Ok(())
    }

    /// [`Runner::touch`] for the site, DOOH, source and impression exts.
    fn touch_other(&mut self, obj: &str, o: &mut Obs) -> TestResult {
        let w = &mut self.w;
        match obj {
            "site" => {
                let e = w.get_site_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                e.set_amp(e.get_amp());
                o.bool(e.dirty());
            }
            "dooh" => {
                let e = w.get_dooh_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                o.bool(e.dirty());
            }
            "source" => {
                let e = w.get_source_ext_pair().0;
                e.set_ext(Some(e.get_ext()));
                e.set_s_chain(e.get_s_chain().cloned());
                o.bool(e.dirty());
            }
            "imp" => {
                for mut iw in w.get_imp() {
                    let e = iw.get_imp_ext_pair().0;
                    e.set_ext(Some(e.get_ext()));
                    e.set_prebid(e.get_prebid().cloned());
                    e.set_tid(e.get_tid().to_owned());
                    o.bool(e.dirty());
                }
            }
            _ => return Err(format!("unknown object {obj}")),
        }
        Ok(())
    }
}

/// A `Clear*` step: the function, then an empty error and the request.
fn clear(w: &mut RequestWrapper, f: fn(&mut RequestWrapper), o: &mut Obs) {
    f(w);
    o.request(None, w);
}
