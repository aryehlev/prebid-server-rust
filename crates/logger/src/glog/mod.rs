//! Go `github.com/golang/glog` v1.2.5: leveled logs to standard error and to per-severity
//! log files, configured by glog's command-line flags.
//!
//! Go: `glog.go`, `glog_file.go`, `glog_flags.go` and `internal/logsink`. Every line is
//! glog's:
//!
//! ```text
//! Lmmdd hh:mm:ss.uuuuuu threadid file:line] msg
//! ```
//!
//! where `L` is the severity letter (`IWEF`), the time is the local wall clock (UTC in
//! Prebid's container, and always UTC here), `threadid` is the process id right-aligned in
//! seven columns, and `file` is the base name of the call site's file. The message is formatted
//! by Go's `fmt` rules, the whole line is cut to 14999 bytes and a newline is added unless the
//! line already ends in one.
//!
//! Lines go to standard error when `-logtostderr` or `-alsologtostderr` is set or the severity
//! is at least `-stderrthreshold` (ERROR by default), and, unless `-logtostderr` is set, to log
//! files: one per severity, created on first use in `-log_dir` (else the temporary directory)
//! as `program.host.user.log.SEVERITY.yyyymmdd-hhmmss.pid`, each receiving its severity and
//! every higher one, with `program.SEVERITY` symbolic links beside them and in `-log_link`.
//! Files are buffered and flushed every 30 seconds, after any line above `-logbuflevel`, and by
//! [`flush`]; they rotate after 1800 MiB.
//!
//! A FATAL line carries a stack trace, then the files are flushed and the process ends through
//! the installed [`ExitFn`](crate::exit::ExitFn) (by default with status 2, the status Go's
//! runtime exits with on glog's `SIGABRT`). A failure to write a line is itself fatal, as in Go.
//!
//! Differences from Go: call sites are Rust files and lines (except for the
//! [`crate`]-level functions, which report Go's); stack traces are Rust backtraces; the file
//! header names the Rust build (`Binary: Built with rustc for linux/amd64`); `-v` and
//! `-vmodule` are parsed and kept but no `V` logging is ported, since no Prebid code uses it;
//! the host name comes from `/proc/sys/kernel/hostname` and the user from `/etc/passwd` or
//! `$USER`, falling back to Go's `unknownhost` and `unknownuser`; and Go's context and
//! structured sinks, `Stats`, `CopyStandardLogTo` and `Exit` functions are not ported.

mod bridge;
mod file;
mod flags;
mod format;
#[cfg(test)]
mod tests;

use std::{
    backtrace::Backtrace,
    env, error, fmt, fs,
    io::{self, Write},
    panic::Location,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, LazyLock, Mutex, Once, OnceLock, PoisonError, RwLock, RwLockReadGuard, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread,
    time::{Duration as StdDuration, Instant},
};

pub use bridge::{GlogBridge, install_log_bridge};
use file::{FileContext, FileSink};
pub use flags::{FlagError, Flags, ModulePattern, SetError, TraceLocation, print_defaults, usage};
use pbs_gocompat::{fmt::Value, strconv, strings, time::Time};

use crate::{exit, goerr};

/// Go: `glog.MaxSize`: the size at which a log file rotates.
pub const MAX_SIZE: u64 = 1024 * 1024 * 1800;

/// Go: `flushInterval` of the flush daemon.
const FLUSH_INTERVAL: StdDuration = StdDuration::from_secs(30);

/// Go: `logsink.Severity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Go: `logsink.Info` (0).
    Info,
    /// Go: `logsink.Warning` (1).
    Warning,
    /// Go: `logsink.Error` (2).
    Error,
    /// Go: `logsink.Fatal` (3).
    Fatal,
}

impl Severity {
    /// Every severity, from INFO to FATAL.
    pub const ALL: [Self; 4] = [Self::Info, Self::Warning, Self::Error, Self::Fatal];

    /// Go: `Severity.String`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }

    /// Go's numeric value, 0 to 3.
    #[must_use]
    pub const fn as_int(self) -> i8 {
        match self {
            Self::Info => 0,
            Self::Warning => 1,
            Self::Error => 2,
            Self::Fatal => 3,
        }
    }

    /// The severity of a numeric value, 0 to 3.
    #[must_use]
    pub const fn from_int(n: i8) -> Option<Self> {
        match n {
            0 => Some(Self::Info),
            1 => Some(Self::Warning),
            2 => Some(Self::Error),
            3 => Some(Self::Fatal),
            _ => None,
        }
    }

    /// The line's first byte (Go: `"IWEF"[severity]`).
    const fn letter(self) -> u8 {
        match self {
            Self::Info => b'I',
            Self::Warning => b'W',
            Self::Error => b'E',
            Self::Fatal => b'F',
        }
    }

    /// The index of the severity's log file.
    const fn index(self) -> usize {
        match self {
            Self::Info => 0,
            Self::Warning => 1,
            Self::Error => 2,
            Self::Fatal => 3,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error of [`parse_severity`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseSeverityError(String);

impl fmt::Display for ParseSeverityError {
    /// Go: `logsink: invalid severity "NAME"`, with the name in upper case.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "logsink: invalid severity {}", strconv::quote(&self.0))
    }
}

impl error::Error for ParseSeverityError {}

/// Go: `logsink.ParseSeverity`: a severity name in any case (Go's Unicode upper-casing, so
/// `ınfo` is INFO).
///
/// # Errors
///
/// When the name is not a severity.
pub fn parse_severity(name: &str) -> Result<Severity, ParseSeverityError> {
    let upper = strings::to_upper(name);
    Severity::ALL
        .into_iter()
        .find(|s| s.as_str() == upper)
        .ok_or(ParseSeverityError(upper))
}

/// Where a log call was made: the file, whose base name glog prints, and the line (Go: the
/// `runtime.Caller` frame glog's depth selects).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Site<'a> {
    /// The source file.
    pub file: &'a str,
    /// The line.
    pub line: u32,
}

impl Site<'static> {
    /// The caller of the `#[track_caller]` function that calls this.
    #[track_caller]
    #[must_use]
    pub const fn caller() -> Self {
        let location = Location::caller();
        Self {
            file: location.file(),
            line: location.line(),
        }
    }
}

impl Site<'_> {
    /// The text after the last `/` of the file.
    fn base_name(&self) -> &str {
        self.file
            .rfind('/')
            .and_then(|i| self.file.get(i + 1..))
            .unwrap_or(self.file)
    }
}

/// The process facts glog writes into file names and headers, and its clock.
#[derive(Clone, Debug)]
pub struct Env {
    /// Go: `filepath.Base(os.Args[0])`.
    pub program: String,
    /// Go: the host name up to its first dot.
    pub host: String,
    /// Go: the user name with every character outside `[A-Za-z0-9]` replaced by `_`.
    pub user: String,
    /// Go: `os.Getpid()`, also printed as each line's thread id.
    pub pid: u32,
    /// Go: `os.TempDir()`, the fallback log directory.
    pub temp_dir: PathBuf,
    /// Go: `timeNow`.
    pub clock: fn() -> Time,
}

impl Env {
    /// The current process's facts and the system clock.
    #[must_use]
    pub fn process() -> Self {
        let argv0 = env::args_os()
            .next()
            .map_or_else(String::new, |a| a.to_string_lossy().into_owned());
        Self {
            program: go_base(&argv0),
            host: host_name(),
            user: sanitize_user(&user_name()),
            pid: process::id(),
            temp_dir: env::temp_dir(),
            clock: Time::now,
        }
    }
}

/// Go: `filepath.Base`.
fn go_base(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    trimmed
        .rfind('/')
        .and_then(|i| trimmed.get(i + 1..))
        .unwrap_or(trimmed)
        .to_owned()
}

/// Go: `shortHostname(os.Hostname())`, or `unknownhost`.
fn host_name() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim_end_matches('\n').to_owned())
        .filter(|h| !h.is_empty())
        .map_or_else(
            || "unknownhost".to_owned(),
            |h| h.split('.').next().unwrap_or_default().to_owned(),
        )
}

/// Go: `user.Current().Username`: the `/etc/passwd` name of this process's user, else `$USER`.
fn user_name() -> String {
    passwd_user().unwrap_or_else(|| env::var("USER").unwrap_or_default())
}

/// The `/etc/passwd` entry for the owner of `/proc/self` (Linux).
#[cfg(unix)]
fn passwd_user() -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;

    let uid = fs::metadata("/proc/self").ok()?.uid().to_string();
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let mut parts = line.split(':');
        let name = parts.next()?;
        (parts.nth(1)? == uid).then(|| name.to_owned())
    })
}

/// No `/etc/passwd` lookup off Unix.
#[cfg(not(unix))]
const fn passwd_user() -> Option<String> {
    None
}

/// Go: glog's mapping of the user name, or `unknownuser` when there is none.
fn sanitize_user(name: &str) -> String {
    if name.is_empty() {
        return "unknownuser".to_owned();
    }
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// A glog logger: its flags, standard error and log files.
///
/// Go has one, in package state; [`global`] is the process's, and tests make their own.
pub struct Glog {
    env: Env,
    flags: RwLock<Flags>,
    stderr: Mutex<Box<dyn Write + Send>>,
    files: Mutex<FileSink>,
    max_size: AtomicU64,
    /// The flush daemon's channel, made with the first log file (Go: `flushChan`).
    flusher: OnceLock<Option<SyncSender<Severity>>>,
    /// Go: `sinkErrOnce`.
    sink_error: Once,
    this: Weak<Self>,
}

impl fmt::Debug for Glog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Glog")
            .field("env", &self.env)
            .field("flags", &*self.read_flags())
            .finish_non_exhaustive()
    }
}

static GLOBAL: LazyLock<Arc<Glog>> =
    LazyLock::new(|| Glog::new(Env::process(), Flags::default(), Box::new(io::stderr())));

/// The process-wide glog, writing to the process's standard error.
#[must_use]
pub fn global() -> &'static Arc<Glog> {
    &GLOBAL
}

impl Glog {
    /// A glog with `flags` writing standard-error lines to `stderr`.
    #[must_use]
    pub fn new(env: Env, flags: Flags, stderr: Box<dyn Write + Send>) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            env,
            flags: RwLock::new(flags),
            stderr: Mutex::new(stderr),
            files: Mutex::new(FileSink::default()),
            max_size: AtomicU64::new(MAX_SIZE),
            flusher: OnceLock::new(),
            sink_error: Once::new(),
            this: Weak::clone(this),
        })
    }

    /// The process facts.
    #[must_use]
    pub const fn env(&self) -> &Env {
        &self.env
    }

    fn read_flags(&self) -> RwLockReadGuard<'_, Flags> {
        self.flags.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current flag values.
    #[must_use]
    pub fn flags(&self) -> Flags {
        self.read_flags().clone()
    }

    /// Replaces every flag value.
    pub fn set_flags(&self, flags: Flags) {
        *self.flags.write().unwrap_or_else(PoisonError::into_inner) = flags;
    }

    /// Go: `flag.Set(name, value)` for one of glog's flags.
    ///
    /// # Errors
    ///
    /// When there is no such flag or the value does not parse; the flag keeps its value.
    pub fn set_flag(&self, name: &str, value: &str) -> Result<(), SetError> {
        self.flags
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .set(name, value)
    }

    /// Go: `flag.Parse()` over `args` (without the program name) for glog's flags; returns
    /// the arguments after the flags.
    ///
    /// # Errors
    ///
    /// On `-h`, `-help`, an undefined flag or a bad value, as Go's flag package; flags before
    /// the failing one keep their new values. [`FlagError::output`] is what Go prints and
    /// [`FlagError::exit_status`] the status Go exits with.
    pub fn parse_flags(&self, args: &[String]) -> Result<Vec<String>, FlagError> {
        self.flags
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .parse(args)
    }

    /// Replaces where standard-error lines go (Go's tests set `sinks.stderr.w`).
    pub fn set_stderr(&self, stderr: Box<dyn Write + Send>) {
        *self.stderr.lock().unwrap_or_else(PoisonError::into_inner) = stderr;
    }

    /// Go: `glog.MaxSize = size`.
    pub fn set_max_size(&self, size: u64) {
        self.max_size.store(size, Ordering::Relaxed);
    }

    /// Go: `glog.XDepthf`: logs `format` with `args` at `site`; FATAL logs a stack trace,
    /// flushes and ends the process.
    pub fn logf(&self, site: Site<'_>, severity: Severity, format: &str, args: &[Value]) {
        if severity == Severity::Fatal {
            self.fatalf(site, format, args);
        }
        self.print(site, severity, format, args, false);
    }

    /// Go: `glog.FatalDepthf`: logs at FATAL with a stack trace, flushes the log files and
    /// ends the process with status 2 through the installed [`ExitFn`](crate::exit::ExitFn).
    pub fn fatalf(&self, site: Site<'_>, format: &str, args: &[Value]) -> ! {
        self.print(site, Severity::Fatal, format, args, true);
        self.flush_and_abort()
    }

    /// Go: `glog.Flush`: writes buffered log file data and syncs the files.
    pub fn flush(&self) {
        self.flush_files(Severity::Info);
    }

    /// Go: `flushAndAbort`.
    fn flush_and_abort(&self) -> ! {
        self.flush();
        exit::exit(2)
    }

    /// Go: `ctxlogf` and `logsink.textPrintf`.
    fn print(
        &self,
        site: Site<'_>,
        severity: Severity,
        format: &str,
        args: &[Value],
        with_stack: bool,
    ) {
        let flags = self.read_flags();
        let to_stderr =
            flags.logtostderr || flags.alsologtostderr || severity >= flags.stderrthreshold;
        let to_files = !flags.logtostderr;
        if !to_stderr && !to_files && severity != Severity::Fatal {
            return;
        }
        let now = (self.env.clock)();
        let line = if with_stack || flags.backtrace_at(site) {
            let mut format = format.to_owned();
            format.push_str("\n\n%v\n");
            let mut args = args.to_vec();
            args.push(Value::String(Backtrace::force_capture().to_string()));
            format::line(severity, &now, self.env.pid, site, &format, &args)
        } else {
            format::line(severity, &now, self.env.pid, site, format, args)
        };
        let mut result = if to_stderr {
            self.write_stderr(&line)
        } else {
            Ok(())
        };
        if to_files {
            let files = self.write_files(&flags, severity, &line);
            result = result.and(files);
        }
        drop(flags);
        if let Err(err) = result {
            self.sink_failed(&err);
        }
    }

    fn write_stderr(&self, line: &[u8]) -> Result<(), String> {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_all(line)
            .map_err(|e| goerr::path_error("write", Path::new("/dev/stderr"), &e))
    }

    /// Go: `fileSink.Emit`.
    fn write_files(&self, flags: &Flags, severity: Severity, line: &[u8]) -> Result<(), String> {
        let ctx = FileContext {
            env: &self.env,
            log_dir: &flags.log_dir,
            log_link: &flags.log_link,
            max_size: self.max_size.load(Ordering::Relaxed),
        };
        let result = self
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .emit(&ctx, severity, line);
        let daemon = self.flusher.get_or_init(|| self.start_flush_daemon());
        if let Some(flusher) = daemon
            && i64::from(severity.as_int()) > flags.logbuflevel
        {
            // Go: a non-blocking send; a pending request already covers this one.
            let _pending = flusher.try_send(severity);
        }
        result
    }

    /// Go: `fileSink.flush(threshold)`: flushes and syncs the files of `threshold` and above.
    fn flush_files(&self, threshold: Severity) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush(threshold);
    }

    /// Go: the fatal message of `sinkf` when a sink fails, written once (to standard error,
    /// which always takes FATAL lines, and to the files unless `-logtostderr`), then the abort.
    fn sink_failed(&self, err: &str) {
        self.sink_error.call_once(|| {
            let site = Site {
                file: "glog.go",
                line: 261,
            };
            let stack = Backtrace::force_capture().to_string();
            let line = format::line(
                Severity::Fatal,
                &(self.env.clock)(),
                self.env.pid,
                site,
                "log: exiting because of error writing previous log to sinks: %v\n\n%v\n",
                &[Value::from(err), Value::String(stack)],
            );
            let _stderr_result = self.write_stderr(&line);
            let flags = self.read_flags();
            if !flags.logtostderr {
                let _files_result = self.write_files(&flags, Severity::Fatal, &line);
            }
            drop(flags);
            self.flush_and_abort();
        });
    }

    /// Go: `go sinks.file.flushDaemon()`.
    fn start_flush_daemon(&self) -> Option<SyncSender<Severity>> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let this = Weak::clone(&self.this);
        let daemon = thread::Builder::new()
            .name("glog-flush".to_owned())
            .spawn(move || flush_daemon(&this, &receiver))
            .ok()?;
        drop(daemon);
        Some(sender)
    }
}

/// Go: `fileSink.flushDaemon`: flushes every file every 30 seconds, and the files at and
/// above a severity when a line above `-logbuflevel` asks; ends with its glog.
fn flush_daemon(glog: &Weak<Glog>, requests: &Receiver<Severity>) {
    let mut tick = Instant::now() + FLUSH_INTERVAL;
    loop {
        let threshold = match requests.recv_timeout(tick.saturating_duration_since(Instant::now()))
        {
            Ok(severity) => severity,
            Err(RecvTimeoutError::Timeout) => {
                tick += FLUSH_INTERVAL;
                Severity::Info
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let Some(glog) = glog.upgrade() else {
            return;
        };
        glog.flush_files(threshold);
    }
}

/// Go: `glog.InfoDepthf` and friends at an explicit site, through the process-wide glog.
pub fn log_at(site: Site<'_>, severity: Severity, format: &str, args: &[Value]) {
    global().logf(site, severity, format, args);
}

/// Go: `glog.Infof`.
#[track_caller]
pub fn infof(format: &str, args: &[Value]) {
    log_at(Site::caller(), Severity::Info, format, args);
}

/// Go: `glog.Warningf`.
#[track_caller]
pub fn warningf(format: &str, args: &[Value]) {
    log_at(Site::caller(), Severity::Warning, format, args);
}

/// Go: `glog.Errorf`.
#[track_caller]
pub fn errorf(format: &str, args: &[Value]) {
    log_at(Site::caller(), Severity::Error, format, args);
}

/// Go: `glog.Fatalf`.
#[track_caller]
pub fn fatalf(format: &str, args: &[Value]) -> ! {
    global().fatalf(Site::caller(), format, args)
}

/// Go: `glog.Info`: the operands as `%v`, a space between two that are not strings.
#[track_caller]
pub fn info(args: &[Value]) {
    log_at(
        Site::caller(),
        Severity::Info,
        &format::default_format(args),
        args,
    );
}

/// Go: `glog.Warning`.
#[track_caller]
pub fn warning(args: &[Value]) {
    log_at(
        Site::caller(),
        Severity::Warning,
        &format::default_format(args),
        args,
    );
}

/// Go: `glog.Error`.
#[track_caller]
pub fn error(args: &[Value]) {
    log_at(
        Site::caller(),
        Severity::Error,
        &format::default_format(args),
        args,
    );
}

/// Go: `glog.Infoln`: the operands as `%v` separated by spaces, then a newline.
#[track_caller]
pub fn infoln(args: &[Value]) {
    log_at(
        Site::caller(),
        Severity::Info,
        &format::ln_format(args),
        args,
    );
}

/// Go: `glog.Warningln`.
#[track_caller]
pub fn warningln(args: &[Value]) {
    log_at(
        Site::caller(),
        Severity::Warning,
        &format::ln_format(args),
        args,
    );
}

/// Go: `glog.Errorln`.
#[track_caller]
pub fn errorln(args: &[Value]) {
    log_at(
        Site::caller(),
        Severity::Error,
        &format::ln_format(args),
        args,
    );
}

/// Go: `glog.Flush`.
pub fn flush() {
    global().flush();
}

/// Go: `flag.Parse()` for glog's flags on the process-wide glog: `args` are the command-line
/// arguments after the program name.
///
/// # Errors
///
/// As [`Glog::parse_flags`]; Go's `flag.Parse` then prints [`FlagError::output`] to standard
/// error and exits with [`FlagError::exit_status`].
pub fn parse_command_line(args: &[String]) -> Result<Vec<String>, FlagError> {
    global().parse_flags(args)
}
