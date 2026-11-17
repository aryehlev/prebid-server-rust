//! The test: parses the golden, starts the servers, replays the cases and compares.

mod client;
mod drain;
mod handlers;

use core::fmt::Write as _;
use std::{collections::BTreeSet, fs, sync::Arc};

use pbs_gocompat::strconv::{quote_bytes, unquote_bytes};
use tokio::sync::Mutex;

use self::{
    client::{Case, Outcome, Step, run_all},
    handlers::{TempDir, start_servers, write_static},
};

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/wire.golden");

/// Runs the tests one at a time: the shutdown scenarios are timed, and the wire corpus keeps
/// many connections busy.
static SERIAL: Mutex<()> = Mutex::const_new(());

/// Cases whose replies differ from Go's, each a documented gap of the port. The test fails
/// when one of them starts to match, so the list stays exact.
const KNOWN_GAPS: &[(&str, &str)] = &[
    (
        "parse header latin1 value",
        "pbs-gocompat's Header holds Rust strings: header bytes that are not UTF-8 are \
         decoded lossily, where Go keeps the raw bytes",
    ),
    (
        "parse latin1 in uri",
        "pbs-gocompat's Url holds Rust strings: a request target that is not UTF-8 is \
         decoded lossily (RequestURI, Path, RawPath)",
    ),
    (
        "parse latin1 escaped in uri",
        "pbs-gocompat's Url holds Rust strings: a path whose percent-decoding is not UTF-8 \
         holds U+FFFD, so RawPath is kept where Go drops it",
    ),
    (
        r#"gen chunked auction trailer "X-T: v\x80\r\n""#,
        "pbs-gocompat's Header holds Rust strings: a trailer value that is not UTF-8 is \
         decoded lossily, where Go keeps the raw bytes",
    ),
    (
        r#"gen chunked /openrtb2/video trailer "X-T: v\x80\r\n""#,
        "pbs-gocompat's Header holds Rust strings: a trailer value that is not UTF-8 is \
         decoded lossily, where Go keeps the raw bytes",
    ),
    (
        r#"gen chunked auction trailer "X-\xffT: v\r\n""#,
        "a Rust error message is a string: the malformed trailer line Go's error quotes is \
         decoded lossily when it is not UTF-8",
    ),
    (
        r#"gen chunked /openrtb2/video trailer "X-\xffT: v\r\n""#,
        "a Rust error message is a string: the malformed trailer line Go's error quotes is \
         decoded lossily when it is not UTF-8",
    ),
    (
        r#"gen chunked /t/body/some trailer "X-\xffT: v\r\n""#,
        "a Rust error message is a string: the malformed trailer line Go's error quotes is \
         decoded lossily when it is not UTF-8",
    ),
    (
        r#"gen chunked /t/body/max trailer "X-\xffT: v\r\n""#,
        "a Rust error message is a string: the malformed trailer line Go's error quotes is \
         decoded lossily when it is not UTF-8",
    ),
];

/// `send "<quoted>"` or `send "<quoted>" * <n>`.
fn parse_send(rest: &str) -> Result<Vec<u8>, String> {
    if let Some((quoted, n)) = rest.rsplit_once(" * ")
        && quoted.ends_with('"')
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
    {
        let data = unquote_bytes(quoted).map_err(|e| format!("{quoted}: {e}"))?;
        let n: usize = n.parse().map_err(|e| format!("{n}: {e}"))?;
        return Ok(data.repeat(n));
    }
    unquote_bytes(rest).map_err(|e| format!("{rest}: {e}"))
}

fn parse_golden(text: &str) -> Result<Vec<Case>, String> {
    let mut cases: Vec<Case> = Vec::new();
    for (no, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || format!("{GOLDEN}:{}: {line}", no + 1);
        let (key, rest) = line.split_once(' ').ok_or_else(bad)?;
        if key == "case" {
            cases.push(Case {
                name: rest.to_owned(),
                ..Case::default()
            });
            continue;
        }
        let case = cases.last_mut().ok_or_else(bad)?;
        match key {
            "server" => rest.clone_into(&mut case.server),
            "wait" => case.wait = rest.parse().map_err(|e| format!("{}: {e}", bad()))?,
            "send" => case.steps.push(Step::Send(parse_send(rest)?)),
            "pause" => {
                let ms = rest.parse().map_err(|e| format!("{}: {e}", bad()))?;
                case.steps.push(Step::Pause(ms));
            }
            "response" => {
                case.response = unquote_bytes(rest).map_err(|e| format!("{}: {e}", bad()))?;
            }
            "end" => rest.clone_into(&mut case.end),
            _ => return Err(bad()),
        }
    }
    Ok(cases)
}

fn shorten(b: &[u8]) -> String {
    const MAX: usize = 1500;
    let q = quote_bytes(b.get(..MAX).unwrap_or(b));
    if b.len() > MAX {
        format!("{q}... ({} bytes)", b.len())
    } else {
        q
    }
}

/// The report of the cases that differ, or `None` when the differences are exactly the
/// known gaps.
fn compare(cases: &[Case], outcomes: &[Outcome]) -> Option<String> {
    let gaps: BTreeSet<&str> = KNOWN_GAPS.iter().map(|(name, _)| *name).collect();
    let mut report = String::new();
    let mut differing = 0;
    let mut fixed = Vec::new();
    for (case, got) in cases.iter().zip(outcomes) {
        let same = got.response == case.response && got.end == case.end;
        let known = gaps.contains(case.name.as_str());
        if same && known {
            fixed.push(case.name.as_str());
        }
        if same || known {
            continue;
        }
        differing += 1;
        if differing <= 40 {
            let _ok = write!(
                report,
                "\n--- case {}\n  go:   {} {}\n  rust: {} {}\n",
                case.name,
                shorten(&case.response),
                case.end,
                shorten(&got.response),
                got.end
            );
        }
    }
    if differing == 0 && fixed.is_empty() {
        return None;
    }
    Some(format!(
        "{differing} of {} cases differ from Go; known gaps that now match: {fixed:?}{report}",
        cases.len()
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn wire_bytes_match_go() -> Result<(), String> {
    let _serial = SERIAL.lock().await;
    let text = fs::read_to_string(GOLDEN).map_err(|e| format!("{GOLDEN}: {e}"))?;
    let cases = parse_golden(&text)?;
    if cases.len() < 1000 {
        return Err(format!("only {} cases in {GOLDEN}", cases.len()));
    }
    let dir = TempDir::create().map_err(|e| format!("temp dir: {e}"))?;
    let static_root = dir.path.join("static");
    write_static(&static_root).map_err(|e| format!("static tree: {e}"))?;
    let servers = start_servers(&static_root, &dir.path.join("p.sock")).await?;
    let targets = Arc::new(servers.targets.clone());
    let cases: Arc<[Case]> = cases.into();
    let outcomes = run_all(targets, Arc::clone(&cases)).await;
    servers.stop().await;
    compare(&cases, &outcomes).map_or(Ok(()), Err)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn shutdown_matches_go() -> Result<(), String> {
    let _serial = SERIAL.lock().await;
    let text = fs::read_to_string(drain::GOLDEN).map_err(|e| format!("{}: {e}", drain::GOLDEN))?;
    let scenarios = drain::parse_golden(&text)?;
    if scenarios.len() < 3 {
        return Err(format!(
            "only {} scenarios in {}",
            scenarios.len(),
            drain::GOLDEN
        ));
    }
    let dir = TempDir::create().map_err(|e| format!("temp dir: {e}"))?;
    let mut observed = Vec::with_capacity(scenarios.len());
    for sc in &scenarios {
        observed.push(drain::run(sc, &dir.path).await?);
    }
    drain::compare(&scenarios, &observed).map_or(Ok(()), Err)
}
