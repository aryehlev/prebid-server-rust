//! The test: parses the golden, starts the servers, replays the cases and compares.

mod client;
mod drain;
mod handlers;

use core::fmt::Write as _;
use std::{collections::BTreeSet, env, fs, sync::Arc};

use pbs_gocompat::strconv::{quote_bytes, unquote_bytes};
use tokio::sync::Mutex;

use self::{
    client::{Case, Outcome, Step, run_all},
    handlers::{TempDir, start_servers, write_static},
};

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/wire.golden");

/// Go's `GOOS` for the OS this test runs on: of a case that answers differently by GOOS, the
/// `expect` block that applies here.
const GOOS: &str = if cfg!(target_os = "macos") {
    "darwin"
} else {
    env::consts::OS
};

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

/// One reply the golden lists: its `response` and `end` lines.
#[derive(Debug, Default)]
struct Expect {
    response: Option<Vec<u8>>,
    end: Option<String>,
}

/// A case as the golden lists it: one reply, or, for a case that answers differently by GOOS,
/// one `expect` block per GOOS.
#[derive(Debug, Default)]
struct Listed {
    case: Case,
    shared: Expect,
    by_goos: Vec<(String, Expect)>,
}

impl Listed {
    /// The reply the `response` and `end` lines belong to: that of the last `expect` block,
    /// else the shared one.
    fn reply(&mut self) -> &mut Expect {
        match self.by_goos.last_mut() {
            Some((_, e)) => e,
            None => &mut self.shared,
        }
    }

    /// The case with the reply Go gives on `goos`: an error for a case with `expect` blocks
    /// but none for `goos`.
    fn resolve(self, goos: &str) -> Result<Case, String> {
        let Self {
            mut case,
            shared,
            by_goos,
        } = self;
        let reply = if by_goos.is_empty() {
            shared
        } else {
            let listed: Vec<&str> = by_goos.iter().map(|(g, _)| g.as_str()).collect();
            let listed = listed.join(", ");
            by_goos
                .into_iter()
                .find_map(|(g, e)| (g == goos).then_some(e))
                .ok_or_else(|| {
                    format!(
                        "{GOLDEN}: case {} has no expect block for GOOS {goos} (only {listed})",
                        case.name
                    )
                })?
        };
        let missing = |line: &str| format!("{GOLDEN}: case {} has no {line} line", case.name);
        let response = reply.response.ok_or_else(|| missing("response"))?;
        let end = reply.end.ok_or_else(|| missing("end"))?;
        case.response = response;
        case.end = end;
        Ok(case)
    }
}

/// The wire golden, with the replies Go gives on one GOOS.
#[derive(Debug)]
struct Golden {
    /// The GOOS the generator ran on: its `goos` line.
    generated_on: String,
    cases: Vec<Case>,
}

/// Parses the golden (Go: `writeGolden`) for `goos`: a case with `expect` blocks gets the reply
/// of the one for `goos`, and the parse fails when it has none.
fn parse_golden(text: &str, goos: &str) -> Result<Golden, String> {
    let mut generated_on = None;
    let mut listed: Vec<Listed> = Vec::new();
    for (no, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |why: &str| format!("{GOLDEN}:{}: {line}: {why}", no + 1);
        let (key, rest) = line.split_once(' ').ok_or_else(|| bad("no value"))?;
        if key == "goos" {
            if generated_on.is_some() || !listed.is_empty() {
                return Err(bad("goos is one line before the first case"));
            }
            generated_on = Some(rest.to_owned());
            continue;
        }
        if key == "case" {
            listed.push(Listed {
                case: Case {
                    name: rest.to_owned(),
                    ..Case::default()
                },
                ..Listed::default()
            });
            continue;
        }
        let l = listed.last_mut().ok_or_else(|| bad("before a case"))?;
        match key {
            "server" => rest.clone_into(&mut l.case.server),
            "wait" => l.case.wait = rest.parse().map_err(|e| bad(&format!("{e}")))?,
            "send" => l.case.steps.push(Step::Send(parse_send(rest)?)),
            "pause" => {
                let ms = rest.parse().map_err(|e| bad(&format!("{e}")))?;
                l.case.steps.push(Step::Pause(ms));
            }
            "expect" => {
                if l.shared.response.is_some() || l.shared.end.is_some() {
                    return Err(bad("an expect block after the case's reply"));
                }
                if l.by_goos.iter().any(|(g, _)| g == rest) {
                    return Err(bad("a second expect block for this GOOS"));
                }
                l.by_goos.push((rest.to_owned(), Expect::default()));
            }
            "response" => {
                let response = unquote_bytes(rest).map_err(|e| bad(&format!("{e}")))?;
                if l.reply().response.replace(response).is_some() {
                    return Err(bad("a second response line"));
                }
            }
            "end" => {
                if l.reply().end.replace(rest.to_owned()).is_some() {
                    return Err(bad("a second end line"));
                }
            }
            _ => return Err(bad("unknown key")),
        }
    }
    let generated_on = generated_on.ok_or_else(|| format!("{GOLDEN}: no goos line"))?;
    let cases = listed
        .into_iter()
        .map(|l| l.resolve(goos))
        .collect::<Result<_, _>>()?;
    Ok(Golden {
        generated_on,
        cases,
    })
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
/// known gaps. `generated_on` is the GOOS the golden was generated on.
fn compare(cases: &[Case], outcomes: &[Outcome], generated_on: &str) -> Option<String> {
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
        "{differing} of {} cases differ from Go on {GOOS} (golden generated on {generated_on}); \
         known gaps that now match: {fixed:?}{report}",
        cases.len()
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn wire_bytes_match_go() -> Result<(), String> {
    let _serial = SERIAL.lock().await;
    let text = fs::read_to_string(GOLDEN).map_err(|e| format!("{GOLDEN}: {e}"))?;
    let Golden {
        generated_on,
        cases,
    } = parse_golden(&text, GOOS)?;
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
    compare(&cases, &outcomes, &generated_on).map_or(Ok(()), Err)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn shutdown_matches_go() -> Result<(), String> {
    let _serial = SERIAL.lock().await;
    let text = fs::read_to_string(drain::GOLDEN).map_err(|e| format!("{}: {e}", drain::GOLDEN))?;
    let (generated_on, scenarios) = drain::parse_golden(&text)?;
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
    drain::compare(&scenarios, &observed).map_or(Ok(()), |report| {
        Err(format!(
            "shutdown differs from Go on {GOOS} (golden generated on {generated_on}):{report}"
        ))
    })
}
