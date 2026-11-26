//! The differences ledger and its generator.
//!
//! Each crate records the observable differences it causes in `crates/<crate>/differences.toml`:
//! `[[difference]]` tables with `id` (dotted, its first segment the category), `category`,
//! `status` (`ledgered`, `restored`, `decision-pending` or `extension`), `title`, `go`, `rust`,
//! `who` (`publisher`, `bidder`, `operator`, `analytics` or `none`), `visible_in`, `pin` (the
//! test that shows both behaviours, or for an extension the test that shows it off is Go,
//! `crates/<crate>/tests/<path>.rs::<test>` or under the crate's `src/`, a test function
//! `cargo test` runs, not `#[ignore]`d), and as the status needs them
//! `restore`, `default`, `decision`, `rule_b` (`a`, `b`, `c`, `d` or `prebid-logic`),
//! `restore_by` and `note`, with `[[difference.harness]]` patterns (`kind`, `lexical_class`,
//! `path` and `input`) that tell the parity harness which mismatches the entry accounts for. A
//! `[[restoration]]` table (`id`, `pin`, `note`) gives a restored entry its restore pin: the
//! test, in the crate that keeps Go's behaviour, that compares it with a Go golden. It may sit in
//! any fragment, so the crate that restores an entry recorded elsewhere says so in its own file.
//!
//! [`write()`] validates every fragment and writes `docs/DIFFERENCES.md` and
//! `docs/differences.json`; [`check()`] validates them and fails unless both files are what it
//! would write. The text fields are one line of Markdown each.

mod fragment;
mod pattern;
mod pins;
mod render;
#[cfg(test)]
mod tests;
mod validate;
mod vocab;

use std::{
    error::Error,
    fmt, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use self::{
    pins::Sources,
    validate::{Ledger, validate},
};

/// The ledger for people, relative to the workspace root.
pub const MARKDOWN: &str = "docs/DIFFERENCES.md";

/// The ledger for the parity harness, relative to the workspace root.
pub const JSON: &str = "docs/differences.json";

/// The name of a crate's fragment, in the crate's directory.
pub const FRAGMENT: &str = "differences.toml";

/// The command that rewrites both files.
const WRITE_COMMAND: &str = "cargo run -p pbs-differences --locked --offline -- --write";

/// The usage of the `pbs-differences` command.
pub const USAGE: &str = "usage: pbs-differences (--write | --check) [--no-pending] [--root DIR]

  --write       validate every crates/*/differences.toml, then write docs/DIFFERENCES.md and
                docs/differences.json
  --check       validate them and fail unless both files are current
  --no-pending  also fail while a restored entry waits for its restore pin
  --root DIR    the workspace root (default: the workspace pbs-differences was built in)
";

/// A failed run: every problem found, one per line.
#[derive(Clone, PartialEq, Eq)]
pub struct LedgerError {
    /// What is wrong, each naming its fragment and entry or its file.
    problems: Vec<String>,
}

impl LedgerError {
    /// An error with one problem.
    fn one<S: Into<String>>(problem: S) -> Self {
        Self {
            problems: vec![problem.into()],
        }
    }

    /// What is wrong, each naming its fragment and entry or its file.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.problems.join("\n"))
    }
}

// `main` returns `Result<(), LedgerError>` and the runtime prints the error with `Debug`; print
// the problems themselves so a failure reads as plain text.
impl fmt::Debug for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for LedgerError {}

/// Whether a restored entry may still wait for its restore pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// It may: the unit that brings the pin has not merged yet.
    Allowed,
    /// It may not: every restore pin has landed (R1's exit gate).
    Refused,
}

/// What became of one generated file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// It already held what the generator writes.
    Current,
    /// The generator wrote it.
    Written,
}

/// What a successful run found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The fragments read.
    fragments: usize,
    /// The entries per status: decision pending, ledgered, restored, extension.
    counts: [usize; 4],
    /// The restored entries still waiting for their restore pin, with the units that bring it.
    pending: Vec<String>,
    /// The generated files, with what became of each.
    files: Vec<(&'static str, FileState)>,
}

impl Summary {
    /// The restored entries still waiting for their restore pin, with the units that bring it.
    #[must_use]
    pub fn pending(&self) -> &[String] {
        &self.pending
    }

    /// The generated files, with what became of each.
    #[must_use]
    pub fn files(&self) -> &[(&'static str, FileState)] {
        &self.files
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [pending, ledgered, restored, extensions] = self.counts;
        let entries = pending + ledgered + restored + extensions;
        writeln!(
            f,
            "{entries} {} from {} {}: {pending} decision pending, {ledgered} ledgered, \
             {restored} Go behaviour kept, {extensions} {}",
            if entries == 1 { "entry" } else { "entries" },
            self.fragments,
            if self.fragments == 1 {
                "fragment"
            } else {
                "fragments"
            },
            if extensions == 1 {
                "extension"
            } else {
                "extensions"
            }
        )?;
        for entry in &self.pending {
            writeln!(f, "restore pin pending: {entry}")?;
        }
        for (file, state) in &self.files {
            let word = match state {
                FileState::Current => "current",
                FileState::Written => "written",
            };
            writeln!(f, "{file}: {word}")?;
        }
        Ok(())
    }
}

/// The workspace `pbs-differences` was built in: two levels above its manifest.
#[must_use]
pub fn workspace_root() -> Option<&'static Path> {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)
}

/// Reads every `crates/*/differences.toml` under `root`, in path order, with its path relative
/// to `root`.
fn read_fragments(root: &Path) -> Result<Vec<(String, String)>, LedgerError> {
    let crates = root.join("crates");
    let listing = fs::read_dir(&crates)
        .map_err(|e| LedgerError::one(format!("{}: {e}", crates.display())))?;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for item in listing {
        let item = item.map_err(|e| LedgerError::one(format!("{}: {e}", crates.display())))?;
        dirs.push(item.path());
    }
    dirs.sort();
    let mut fragments = Vec::new();
    for dir in dirs.iter().filter(|dir| dir.is_dir()) {
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let path = dir.join(FRAGMENT);
        match fs::read_to_string(&path) {
            Ok(text) => fragments.push((format!("crates/{name}/{FRAGMENT}"), text)),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(LedgerError::one(format!("{}: {e}", path.display()))),
        }
    }
    Ok(fragments)
}

/// Reads and validates the ledger of the workspace at `root`.
fn load(root: &Path) -> Result<Ledger, LedgerError> {
    let mut problems = Vec::new();
    let mut parsed = Vec::new();
    for (path, text) in read_fragments(root)? {
        match fragment::parse(&text) {
            Ok(fragment) => parsed.push((path, fragment)),
            Err(e) => problems.push(format!("{path}: {}", e.trim_end())),
        }
    }
    let mut sources = Sources::new(root);
    match validate(parsed, &mut sources) {
        Ok(ledger) if problems.is_empty() => Ok(ledger),
        Ok(_) => Err(LedgerError { problems }),
        Err(more) => {
            problems.extend(more);
            Err(LedgerError { problems })
        }
    }
}

/// What the ledger generates: its summary, and each file with its text.
#[derive(Debug)]
struct Generated {
    /// The summary, with no file states yet.
    summary: Summary,
    /// `docs/DIFFERENCES.md` and `docs/differences.json`, each with its text.
    files: [(&'static str, String); 2],
}

/// The two files the ledger of the workspace at `root` generates, with the ledger's summary.
fn generate(root: &Path, pending: Pending) -> Result<Generated, LedgerError> {
    let ledger = load(root)?;
    let waiting: Vec<String> = ledger
        .entries
        .iter()
        .filter(|entry| entry.restore_pending())
        .map(|entry| format!("{} ({})", entry.id, entry.restore_by.join(", ")))
        .collect();
    if pending == Pending::Refused && !waiting.is_empty() {
        return Err(LedgerError {
            problems: waiting
                .iter()
                .map(|entry| format!("restore pin pending: {entry}"))
                .collect(),
        });
    }
    let json = render::json(&ledger).map_err(|e| LedgerError::one(format!("{JSON}: {e}")))?;
    let mut counts = [0; 4];
    for (count, status) in counts.iter_mut().zip(vocab::Status::ALL) {
        *count = ledger
            .entries
            .iter()
            .filter(|entry| entry.status == status)
            .count();
    }
    let summary = Summary {
        fragments: ledger.fragments.len(),
        counts,
        pending: waiting,
        files: Vec::new(),
    };
    Ok(Generated {
        summary,
        files: [(MARKDOWN, render::markdown(&ledger)), (JSON, json)],
    })
}

/// What `path` under `root` holds, `None` when it does not exist.
fn current(root: &Path, path: &str) -> Result<Option<String>, LedgerError> {
    let file = root.join(path);
    match fs::read_to_string(&file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(LedgerError::one(format!("{}: {e}", file.display()))),
    }
}

/// Validates the ledger of the workspace at `root` and fails unless `docs/DIFFERENCES.md` and
/// `docs/differences.json` hold what [`write()`] would write.
///
/// # Errors
///
/// Every problem of the fragments; else each generated file that is missing or stale, and, with
/// [`Pending::Refused`], each restored entry still waiting for its restore pin.
pub fn check(root: &Path, pending: Pending) -> Result<Summary, LedgerError> {
    let Generated { mut summary, files } = generate(root, pending)?;
    let mut problems = Vec::new();
    for (path, text) in files {
        match current(root, path)? {
            Some(on_disk) if on_disk == text => summary.files.push((path, FileState::Current)),
            Some(_) => problems.push(format!("{path} is stale: run {WRITE_COMMAND}")),
            None => problems.push(format!("{path} is missing: run {WRITE_COMMAND}")),
        }
    }
    if problems.is_empty() {
        Ok(summary)
    } else {
        Err(LedgerError { problems })
    }
}

/// Validates the ledger of the workspace at `root` and writes `docs/DIFFERENCES.md` and
/// `docs/differences.json`, leaving a file that already holds the right text untouched.
///
/// # Errors
///
/// Every problem of the fragments, in which case nothing is written; with [`Pending::Refused`],
/// each restored entry still waiting for its restore pin; or the file system's error.
pub fn write(root: &Path, pending: Pending) -> Result<Summary, LedgerError> {
    let Generated { mut summary, files } = generate(root, pending)?;
    for (path, text) in files {
        if current(root, path)?.as_deref() == Some(text.as_str()) {
            summary.files.push((path, FileState::Current));
            continue;
        }
        let file = root.join(path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| LedgerError::one(format!("{}: {e}", dir.display())))?;
        }
        fs::write(&file, text).map_err(|e| LedgerError::one(format!("{}: {e}", file.display())))?;
        summary.files.push((path, FileState::Written));
    }
    Ok(summary)
}

/// The command line: `--write` or `--check`, then options.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Print the usage.
    Help,
    /// Write the files.
    Write(PathBuf, Pending),
    /// Check the files.
    Check(PathBuf, Pending),
}

/// Parses the arguments after the program name.
fn parse_args(args: &[String]) -> Result<Command, LedgerError> {
    let usage = |problem: &str| LedgerError::one(format!("{problem}\n{}", USAGE.trim_end()));
    let mut mode = None;
    let mut pending = Pending::Allowed;
    let mut root = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => return Ok(Command::Help),
            "--write" | "--check" if mode.is_some() => {
                return Err(usage("give one of --write and --check"));
            }
            "--write" | "--check" => mode = Some(arg.clone()),
            "--no-pending" => pending = Pending::Refused,
            "--root" => {
                let dir = rest
                    .next()
                    .ok_or_else(|| usage("--root needs a directory"))?;
                root = Some(PathBuf::from(dir));
            }
            other => return Err(usage(&format!("unknown argument {other:?}"))),
        }
    }
    let root = match root {
        Some(dir) => dir,
        None => workspace_root()
            .map(Path::to_path_buf)
            .ok_or_else(|| LedgerError::one("cannot find the workspace root: give --root"))?,
    };
    match mode.as_deref() {
        Some("--write") => Ok(Command::Write(root, pending)),
        Some(_) => Ok(Command::Check(root, pending)),
        None => Err(usage("give one of --write and --check")),
    }
}

/// Runs the `pbs-differences` command with `args` (after the program name), reporting to `out`.
///
/// # Errors
///
/// When the arguments are wrong, the ledger is invalid, a generated file is stale (`--check`) or
/// a file cannot be read or written.
pub fn run<W: Write>(args: &[String], out: &mut W) -> Result<(), LedgerError> {
    let report = match parse_args(args)? {
        Command::Help => USAGE.to_owned(),
        Command::Write(root, pending) => write(&root, pending)?.to_string(),
        Command::Check(root, pending) => check(&root, pending)?.to_string(),
    };
    out.write_all(report.as_bytes())
        .map_err(|e| LedgerError::one(format!("writing the report: {e}")))
}
