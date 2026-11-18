//! Test support: the goldens written by `testdata/go/golden`, checks that return errors
//! instead of panicking, a shared output buffer, temporary directories and fixed clocks.
//!
//! Each golden line is a tab-separated list of `strconv.Quote`d fields.

use std::{
    env,
    error::Error,
    fmt::{Debug, Display},
    fs, io, mem,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use pbs_gocompat::{
    strconv::unquote,
    time::{DateFields, Location, Time},
};

/// The result type of every test in this crate.
pub type TestResult = Result<(), Box<dyn Error>>;

/// Reads `testdata/golden/<name>` into rows of unquoted fields.
///
/// # Errors
///
/// When the file cannot be read or a field is not a Go string literal.
pub fn read_golden(name: &str) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "testdata", "golden", name]
        .iter()
        .collect();
    let text = fs::read_to_string(&path)?;
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let row = line
            .split('\t')
            .map(|field| unquote(field).map_err(|e| format!("{name}:{}: {e}: {field}", n + 1)))
            .collect::<Result<Vec<String>, String>>()?;
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(format!("{name} has no rows").into());
    }
    Ok(rows)
}

/// The first `N` fields of `row`.
///
/// # Errors
///
/// When the row is shorter.
pub fn fields<const N: usize>(row: &[String]) -> Result<[&str; N], Box<dyn Error>> {
    let got: Vec<&str> = row.iter().take(N).map(String::as_str).collect();
    got.try_into().map_err(|short: Vec<&str>| {
        format!("row has {} of {N} fields: {row:?}", short.len()).into()
    })
}

/// Fails with `what` unless `cond` holds.
///
/// # Errors
///
/// When `cond` is false.
pub fn check<W: Display>(cond: bool, what: W) -> TestResult {
    if cond {
        Ok(())
    } else {
        Err(format!("check failed: {what}").into())
    }
}

/// Fails unless `got == want`.
///
/// # Errors
///
/// When they differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized, W: Display>(
    got: &T,
    want: &T,
    what: W,
) -> TestResult {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}:\n got: {got:?}\nwant: {want:?}").into())
    }
}

/// An in-memory writer whose clones share their bytes.
#[derive(Clone, Debug, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    /// Takes the bytes written so far, as text.
    #[must_use]
    pub fn take(&self) -> String {
        let bytes = mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner));
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl io::Write for SharedBuf {
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

/// A directory under the temporary directory, removed on drop; its path has no symbolic
/// links, as Go's `filepath.EvalSymlinks` leaves the generator's.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates a fresh directory whose name starts with `tag`.
    ///
    /// # Errors
    ///
    /// When it cannot be created.
    pub fn new(tag: &str) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("{tag}-{}-{n}", process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(fs::canonicalize(path)?))
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

/// 2009-01-23 01:23:23.123123 UTC, the time of Go's chasex documentation example.
#[must_use]
pub fn fixed_time() -> Time {
    Time::date(
        &DateFields {
            year: 2009,
            month: 1,
            day: 23,
            hour: 1,
            minute: 23,
            second: 23,
            nanosecond: 123_123_000,
        },
        Location::Utc,
    )
}
