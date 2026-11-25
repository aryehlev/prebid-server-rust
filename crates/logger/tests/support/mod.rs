//! What the test files share.
//!
//! The Go goldens of `testdata/golden` (written by `testdata/go`), checks that return errors, the
//! facade lines the sinks golden logs, a writer that keeps what it is given, a reader of
//! tracing-subscriber's lines, and fresh directories.

use std::{
    env,
    error::Error,
    ffi::OsString,
    fmt::Debug,
    fs, io,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use pbs_logger::{FATAL_TARGET, debugf, errorf, infof, warnf};
use serde_json::Value;
use tracing_subscriber::fmt::MakeWriter;

/// A test's result; the error is the failure message.
pub type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// Fails with `message` unless `condition` holds.
///
/// # Errors
///
/// `condition` is false.
pub fn check(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

/// Fails, showing both values, unless they are equal.
///
/// # Errors
///
/// They differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(actual: &T, expected: &T, what: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}:\n  got  {actual:?}\n  want {expected:?}").into())
    }
}

/// The golden `name` from `testdata/golden`.
///
/// # Errors
///
/// The file cannot be read or is not JSON.
pub fn golden(name: &str) -> TestResult<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/golden")
        .join(name);
    let data = fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(serde_json::from_slice(&data)?)
}

/// The array at `pointer` in `value`.
///
/// # Errors
///
/// There is no array there.
pub fn list<'v>(value: &'v Value, pointer: &str) -> TestResult<&'v [Value]> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("no array at {pointer}").into())
}

/// The string at `pointer` in `value`.
///
/// # Errors
///
/// There is no string there.
pub fn text<'v>(value: &'v Value, pointer: &str) -> TestResult<&'v str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no string at {pointer} in {value}").into())
}

/// The strings of the array at `pointer` in `value`.
///
/// # Errors
///
/// There is no array of strings there.
pub fn strings(value: &Value, pointer: &str) -> TestResult<Vec<String>> {
    list(value, pointer)?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("not a string at {pointer}: {item}").into())
        })
        .collect()
}

/// A command line: the program, then `args`.
pub fn command_line<S: AsRef<str>>(args: &[S]) -> Vec<OsString> {
    let mut all = vec![OsString::from("prebid-server")];
    all.extend(args.iter().map(|arg| OsString::from(arg.as_ref())));
    all
}

/// The target the tests log at in place of Prebid Server's code: a `pbs_` crate's, which the
/// subscriber keeps from INFO up where it keeps other crates' lines from WARN up.
pub const PREBID: &str = "pbs_server";

/// Logs the sinks golden's four lines (`testdata/go/sinks/child.go`) through the facade, as
/// Prebid Server's code.
pub fn log_levels() {
    debugf!(target: PREBID, "debugf {}", 1);
    infof!(target: PREBID, "infof {}", 2);
    warnf!(target: PREBID, "warnf {}", 3);
    errorf!(target: PREBID, "errorf {}", 4);
}

/// The run of the sinks golden with `scenario` and `args`.
///
/// # Errors
///
/// The golden has no such run.
pub fn sinks_run<'v>(doc: &'v Value, scenario: &str, args: &[&str]) -> TestResult<&'v Value> {
    for run in list(doc, "/runs")? {
        if text(run, "/scenario")? == scenario && strings(run, "/args")? == args {
            return Ok(run);
        }
    }
    Err(format!("the sinks golden has no {scenario} run with {args:?}").into())
}

/// The glog lines among the entries at `pointer` of a sinks run: severity letter and message.
///
/// # Errors
///
/// The entries are not the golden's.
pub fn glog_lines(run: &Value, pointer: &str) -> TestResult<Vec<(String, String)>> {
    let mut lines = Vec::new();
    for entry in list(run, pointer)? {
        if text(entry, "/kind")? == "glog" {
            lines.push((
                text(entry, "/severity")?.to_owned(),
                text(entry, "/message")?.to_owned(),
            ));
        }
    }
    Ok(lines)
}

/// One line tracing-subscriber's fmt layer wrote: an RFC 3339 time, the level right-aligned in
/// five characters, the target, `: ` and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// `INFO`, `WARN` or `ERROR`.
    pub level: String,
    /// The target.
    pub target: String,
    /// The message.
    pub message: String,
}

impl Written {
    /// glog's letter for the line's severity: `I`, `W`, `E`, or `F` for an ERROR at
    /// [`FATAL_TARGET`].
    #[must_use]
    pub fn letter(&self) -> String {
        match (self.level.as_str(), self.target.as_str()) {
            ("ERROR", FATAL_TARGET) => "F",
            ("ERROR", _) => "E",
            ("WARN", _) => "W",
            _ => "I",
        }
        .to_owned()
    }

    /// The letter and the message, as [`glog_lines`] gives a glog line.
    #[must_use]
    pub fn glog_like(&self) -> (String, String) {
        (self.letter(), self.message.clone())
    }
}

/// The lines in `output`, each checked to be in tracing-subscriber's format.
///
/// # Errors
///
/// A line is in another format.
pub fn written(output: &str) -> TestResult<Vec<Written>> {
    output
        .lines()
        .map(|line| {
            let malformed = || format!("not a tracing-subscriber line: {line:?}");
            let (time, rest) = line.split_once(' ').ok_or_else(malformed)?;
            let bytes = time.as_bytes();
            if bytes.len() != 27 || bytes.get(10) != Some(&b'T') || bytes.last() != Some(&b'Z') {
                return Err(malformed().into());
            }
            let (level, rest) = rest.trim_start().split_once(' ').ok_or_else(malformed)?;
            let (target, message) = rest.split_once(": ").ok_or_else(malformed)?;
            Ok(Written {
                level: level.to_owned(),
                target: target.to_owned(),
                message: message.to_owned(),
            })
        })
        .collect()
}

/// A writer that keeps what it is given; clones share the bytes.
#[derive(Debug, Clone, Default)]
pub struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    /// What was written, as text.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

impl io::Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'w> MakeWriter<'w> for Buffer {
    type Writer = Self;

    fn make_writer(&'w self) -> Self::Writer {
        self.clone()
    }
}

/// A new empty directory under the temp directory, removed when dropped.
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// Makes the directory.
    ///
    /// # Errors
    ///
    /// It cannot be made.
    pub fn new() -> TestResult<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = env::temp_dir().join(format!(
            "pbs-logger-test-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The names of the entries of `dir`, sorted.
    ///
    /// # Errors
    ///
    /// `dir` cannot be read.
    pub fn names(dir: &Path) -> TestResult<Vec<String>> {
        let mut names = fs::read_dir(dir)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<TestResult<Vec<String>>>()?;
        names.sort();
        Ok(names)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap_or_default();
    }
}
