//! Go `github.com/chasex/glog`: a leveled logger writing to a file that rotates by size,
//! hour or day; Prebid's filesystem analytics module writes its events with it.
//!
//! Go: `glog.go` of `github.com/chasex/glog` at `c62392af379c`. Each line is the prefix the
//! [flags](L_STD_FLAGS) select, the message and a newline unless the message ends in one:
//!
//! ```text
//! 2009/01/23 01:23:23 DEBUG message
//! ```
//!
//! Lines are buffered and flushed every 30 seconds and by [`Logger::flush`]. The file name has
//! a suffix by rotation mode: none, `-YYYYMMDD-HHMMSS` (by size), `-YYYYMMDD-HH` (by hour) or
//! `-YYYYMMDD` (by day), in local time (UTC here, as in Prebid's container).
//!
//! Not ported, as Prebid never calls them: the `Fatal` and `Panic` methods. The file name of
//! `Lshortfile` and `Llongfile` is the Rust caller's.

#[cfg(test)]
mod tests;

use std::{
    error, fmt,
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write as _},
    panic::Location,
    path::Path,
    sync::{Arc, Mutex, PoisonError, Weak},
    thread,
    time::Duration,
};

use pbs_gocompat::{
    fmt::{Value, sprint, sprintf, sprintln},
    time::Time,
};

use crate::{exit, goerr};

/// Go: `Ldate`: the date, `2009/01/23`.
pub const L_DATE: i32 = 1;
/// Go: `Ltime`: the time, `01:23:23`.
pub const L_TIME: i32 = 2;
/// Go: `Lmicroseconds`: the time with microseconds, `01:23:23.123123`.
pub const L_MICROSECONDS: i32 = 4;
/// Go: `Llongfile`: the full file name and line, `/a/b/c/d.go:23`.
pub const L_LONGFILE: i32 = 8;
/// Go: `Lshortfile`: the file's base name and line, `d.go:23`; overrides `Llongfile`.
pub const L_SHORTFILE: i32 = 16;
/// Go: `LUTC`: the date and time in UTC.
pub const L_UTC: i32 = 32;
/// Go: `Llevel`: the level name, `DEBUG`.
pub const L_LEVEL: i32 = 64;
/// Go: `LstdFlags`: date, time and level.
pub const L_STD_FLAGS: i32 = L_DATE | L_TIME | L_LEVEL;
/// Go: `LstdNull`: no prefix.
pub const L_STD_NULL: i32 = 0;

/// Go: `bufferSize`.
const BUFFER_SIZE: usize = 256 * 1024;

/// Go: `flushInterval`.
const FLUSH_INTERVAL: Duration = Duration::from_secs(30);

/// Go: `glog.Level`: a line's level and a logger's threshold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Go: `Ldebug`.
    #[default]
    Debug,
    /// Go: `Linfo`.
    Info,
    /// Go: `Lwarn`.
    Warn,
    /// Go: `Lerror`.
    Error,
    /// Go: `Lfatal`.
    Fatal,
    /// Go: `Lpanic`.
    Panic,
}

impl Level {
    /// Go: `levelName[level]`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
            Self::Panic => "PANIC",
        }
    }

    /// Go's numeric value, 0 to 5.
    #[must_use]
    pub const fn as_int(self) -> i64 {
        match self {
            Self::Debug => 0,
            Self::Info => 1,
            Self::Warn => 2,
            Self::Error => 3,
            Self::Fatal => 4,
            Self::Panic => 5,
        }
    }
}

/// Go: `glog.RotateMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RotateMode {
    /// Go: `R_None`: never rotate.
    #[default]
    None,
    /// Go: `R_Size`: rotate when a line would pass `maxsize` bytes.
    Size,
    /// Go: `R_Hour`: rotate when the hour changes.
    Hour,
    /// Go: `R_Day`: rotate when the day of the month changes.
    Day,
}

/// Go: `glog.LogOptions`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogOptions {
    /// Go: `File`: the base name of the log file.
    pub file: String,
    /// Go: `Flag`: the prefix flags, such as [`L_STD_FLAGS`].
    pub flag: i32,
    /// Go: `Level`: the lowest level written.
    pub level: Level,
    /// Go: `Mode`.
    pub mode: RotateMode,
    /// Go: `Maxsize`: the size limit of [`RotateMode::Size`].
    pub maxsize: u64,
}

/// The error of [`Logger::new`]: the log file could not be opened.
#[derive(Debug)]
pub struct OpenError {
    /// Go's text, such as `open x.log: permission denied`.
    text: String,
    source: io::Error,
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl error::Error for OpenError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The open file and rotation state (Go: the fields `mu` protects).
#[derive(Debug, Default)]
struct State {
    out: Option<BufWriter<File>>,
    nbytes: u64,
    hour: i64,
    day: i64,
}

#[derive(Debug)]
struct Inner {
    options: LogOptions,
    clock: fn() -> Time,
    state: Mutex<State>,
}

/// Go: `*glog.Logger`; clones share the file.
#[derive(Clone, Debug)]
pub struct Logger {
    inner: Arc<Inner>,
}

/// Go: `itoa`: `i` in decimal, zero-padded to `width` digits.
fn itoa(buf: &mut Vec<u8>, i: i64, width: usize) {
    buf.extend_from_slice(format!("{i:0width$}").as_bytes());
}

/// Go: `createFile`: opens (appending) the file for the time `t` and resets the rotation
/// state.
fn create_file(options: &LogOptions, state: &mut State, t: &Time) -> Result<(), OpenError> {
    let (year, month, day) = (t.year(), t.month(), t.day());
    let (hour, min, sec) = (t.hour(), t.minute(), t.second());
    let base = &options.file;
    let name = match options.mode {
        RotateMode::Size => {
            format!("{base}-{year:04}{month:02}{day:02}-{hour:02}{min:02}{sec:02}")
        }
        RotateMode::Hour => format!("{base}-{year:04}{month:02}{day:02}-{hour:02}"),
        RotateMode::Day => format!("{base}-{year:04}{month:02}{day:02}"),
        RotateMode::None => base.clone(),
    };
    if let Some(mut old) = state.out.take() {
        let _flushed = old.flush();
    }
    let mut open = OpenOptions::new();
    let _options = open.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let _mode = open.mode(0o664);
    }
    let file = open.open(&name).map_err(|source| OpenError {
        text: goerr::path_error("open", Path::new(&name), &source),
        source,
    })?;
    state.out = Some(BufWriter::with_capacity(BUFFER_SIZE, file));
    state.nbytes = 0;
    state.hour = hour;
    state.day = day;
    Ok(())
}

/// Go: `flushDaemon`: flushes every 30 seconds while the logger lives.
fn flush_daemon(inner: &Weak<Inner>) {
    loop {
        thread::sleep(FLUSH_INTERVAL);
        let Some(inner) = inner.upgrade() else {
            return;
        };
        inner.flush();
    }
}

impl Inner {
    /// Go: `formatHeader`.
    fn format_header(&self, buf: &mut Vec<u8>, level: Level, site: &Location<'_>, t: &Time) {
        let flag = self.options.flag;
        let t = if flag & L_UTC == 0 {
            t.clone()
        } else {
            t.utc()
        };
        if flag & (L_DATE | L_TIME | L_MICROSECONDS) != 0 {
            if flag & L_DATE != 0 {
                itoa(buf, t.year(), 4);
                buf.push(b'/');
                itoa(buf, t.month(), 2);
                buf.push(b'/');
                itoa(buf, t.day(), 2);
                buf.push(b' ');
            }
            if flag & (L_TIME | L_MICROSECONDS) != 0 {
                itoa(buf, t.hour(), 2);
                buf.push(b':');
                itoa(buf, t.minute(), 2);
                buf.push(b':');
                itoa(buf, t.second(), 2);
                if flag & L_MICROSECONDS != 0 {
                    buf.push(b'.');
                    itoa(buf, t.nanosecond() / 1000, 6);
                }
                buf.push(b' ');
            }
        }
        if flag & (L_SHORTFILE | L_LONGFILE) != 0 {
            let file = site.file();
            let file = if flag & L_SHORTFILE == 0 {
                file
            } else {
                file.rfind('/')
                    .filter(|&i| i > 0)
                    .and_then(|i| file.get(i + 1..))
                    .unwrap_or(file)
            };
            buf.extend_from_slice(file.as_bytes());
            buf.push(b':');
            buf.extend_from_slice(site.line().to_string().as_bytes());
            buf.push(b' ');
        }
        if flag & L_LEVEL != 0 {
            buf.extend_from_slice(level.as_str().as_bytes());
            buf.push(b' ');
        }
    }

    /// Go: `Output`: formats the line, rotates the file when its mode asks, and buffers the
    /// line. A failure to open the rotated file ends the process with status 2, as in Go.
    fn output(&self, level: Level, site: &Location<'_>, s: &str) {
        let now = (self.clock)();
        let mut buf = Vec::with_capacity(64 + s.len());
        self.format_header(&mut buf, level, site, &now);
        buf.extend_from_slice(s.as_bytes());
        if !s.ends_with('\n') {
            buf.push(b'\n');
        }
        let len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let rotate = match self.options.mode {
            RotateMode::Size => state.nbytes.saturating_add(len) > self.options.maxsize,
            RotateMode::Hour => state.hour != now.hour() || state.day != now.day(),
            RotateMode::Day => state.day != now.day(),
            RotateMode::None => false,
        };
        if rotate && let Err(err) = create_file(&self.options, &mut state, &now) {
            let _written = io::stderr()
                .write_all(format!("log: exiting because of error: {err}\n").as_bytes());
            exit::exit(2);
        }
        if let Some(out) = state.out.as_mut() {
            let _written = out.write_all(&buf);
        }
        state.nbytes = state.nbytes.saturating_add(len);
    }

    /// Go: `Flush`: writes the buffer and syncs the file.
    fn flush(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(out) = state.out.as_mut() {
            let _flushed = out.flush();
            let _synced = out.get_ref().sync_all();
        }
    }

    /// Writes the line when `level` reaches the logger's threshold.
    fn log<F: FnOnce() -> String>(&self, level: Level, site: &Location<'_>, text: F) {
        if self.options.level <= level {
            self.output(level, site, &text());
        }
    }
}

impl Logger {
    /// Go: `glog.New`: opens the file and starts the flush daemon.
    ///
    /// # Errors
    ///
    /// When the file cannot be opened; the text is Go's, such as
    /// `open x.log-20240101: no such file or directory`.
    pub fn new(options: LogOptions) -> Result<Self, OpenError> {
        Self::with_clock(options, Time::now)
    }

    /// [`Logger::new`] with a clock other than the system's.
    ///
    /// # Errors
    ///
    /// As [`Logger::new`].
    pub fn with_clock(options: LogOptions, clock: fn() -> Time) -> Result<Self, OpenError> {
        let mut state = State::default();
        create_file(&options, &mut state, &clock())?;
        let inner = Arc::new(Inner {
            options,
            clock,
            state: Mutex::new(state),
        });
        let weak = Arc::downgrade(&inner);
        if let Ok(daemon) = thread::Builder::new()
            .name("chasex-glog-flush".to_owned())
            .spawn(move || flush_daemon(&weak))
        {
            drop(daemon);
        }
        Ok(Self { inner })
    }

    /// Go: `Flush`.
    pub fn flush(&self) {
        self.inner.flush();
    }

    /// Go: `Debug`: the operands as `fmt.Sprint` prints them.
    #[track_caller]
    pub fn debug(&self, args: &[Value]) {
        self.inner
            .log(Level::Debug, Location::caller(), || sprint(args));
    }

    /// Go: `Debugf`.
    #[track_caller]
    pub fn debugf(&self, format: &str, args: &[Value]) {
        self.inner
            .log(Level::Debug, Location::caller(), || sprintf(format, args));
    }

    /// Go: `Debugln`.
    #[track_caller]
    pub fn debugln(&self, args: &[Value]) {
        self.inner
            .log(Level::Debug, Location::caller(), || sprintln(args));
    }

    /// Go: `Info`.
    #[track_caller]
    pub fn info(&self, args: &[Value]) {
        self.inner
            .log(Level::Info, Location::caller(), || sprint(args));
    }

    /// Go: `Infof`.
    #[track_caller]
    pub fn infof(&self, format: &str, args: &[Value]) {
        self.inner
            .log(Level::Info, Location::caller(), || sprintf(format, args));
    }

    /// Go: `Infoln`.
    #[track_caller]
    pub fn infoln(&self, args: &[Value]) {
        self.inner
            .log(Level::Info, Location::caller(), || sprintln(args));
    }

    /// Go: `Warn`.
    #[track_caller]
    pub fn warn(&self, args: &[Value]) {
        self.inner
            .log(Level::Warn, Location::caller(), || sprint(args));
    }

    /// Go: `Warnf`.
    #[track_caller]
    pub fn warnf(&self, format: &str, args: &[Value]) {
        self.inner
            .log(Level::Warn, Location::caller(), || sprintf(format, args));
    }

    /// Go: `Warnln`.
    #[track_caller]
    pub fn warnln(&self, args: &[Value]) {
        self.inner
            .log(Level::Warn, Location::caller(), || sprintln(args));
    }

    /// Go: `Error`.
    #[track_caller]
    pub fn error(&self, args: &[Value]) {
        self.inner
            .log(Level::Error, Location::caller(), || sprint(args));
    }

    /// Go: `Errorf`.
    #[track_caller]
    pub fn errorf(&self, format: &str, args: &[Value]) {
        self.inner
            .log(Level::Error, Location::caller(), || sprintf(format, args));
    }

    /// Go: `Errorln`.
    #[track_caller]
    pub fn errorln(&self, args: &[Value]) {
        self.inner
            .log(Level::Error, Location::caller(), || sprintln(args));
    }
}
