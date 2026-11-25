//! Glue (Rule B (c): CLI flags): where a line goes is what glog's flags mean (glog v1.2.5's
//! `stderrSink.Enabled` and `fileSink.Enabled`, `glog_file.go:198-236`): with `-logtostderr` every
//! line goes to stderr and none to a file; with `-alsologtostderr` every line goes to stderr and
//! the file; otherwise the file takes every line and stderr the lines at or above
//! `-stderrthreshold` (ledger: `logging.stderr-threshold`, restored). The lines are
//! tracing-subscriber's (ledger: `logging.line-format`), the file is tracing-appender's daily file
//! in `-log_dir` or the default directory (ledger: `logging.files`), and crates other than Prebid
//! Server's own log at WARN and above (ledger: `logging.std-log-lines`).

use std::{
    error::Error,
    ffi::OsString,
    fmt,
    io::Write as _,
    path::{Path, PathBuf},
};

use tracing::{Subscriber, level_filters::LevelFilter};
use tracing_appender::{
    non_blocking::{NonBlockingBuilder, WorkerGuard},
    rolling::{InitError, RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    Layer as _, Registry,
    filter::{Targets, filter_fn},
    fmt::{self as format, MakeWriter},
    layer::SubscriberExt as _,
    util::SubscriberInitExt as _,
};

use crate::{Exit, GlogFlags, Severity, parse_command_line};

/// The start of the targets of Prebid Server's crates, whose lines are kept from INFO up.
const PREBID_TARGETS: &str = "pbs_";

/// The program name the log file is named after when the command line has none.
const DEFAULT_PROGRAM: &str = "prebid-server";

/// Keeps the log file's writer running: dropping it writes out what is buffered.
#[derive(Debug)]
#[must_use = "dropping Logging stops the log file's writer"]
pub struct Logging {
    _file_writer: Option<WorkerGuard>,
}

/// The log file could not be opened. glog falls back to the temp directory and, when no file
/// can be made, exits 2 at the first line it logs; here startup stops with exit code 2 (ledger:
/// `logging.files`).
#[derive(Debug)]
pub struct LogFileError {
    dir: PathBuf,
    source: InitError,
}

impl fmt::Display for LogFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "log: cannot create log in {}: {}",
            self.dir.display(),
            self.source
        )
    }
}

impl Error for LogFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// The subscriber `flags` ask for.
///
/// It writes lines in tracing-subscriber's format to `stderr` and to `<program>.<yyyy-mm-dd>.log`
/// in `-log_dir`, or in `default_log_dir` (glog's `os.TempDir()`) when `-log_dir` is empty.
/// [`Logging`] keeps the file's writer running.
///
/// # Errors
///
/// [`LogFileError`] when the log file's directory cannot be made or the file opened.
pub fn subscriber<W>(
    flags: &GlogFlags,
    program: &str,
    default_log_dir: &Path,
    stderr: W,
) -> Result<(Box<dyn Subscriber + Send + Sync>, Logging), LogFileError>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let every_line = flags.logtostderr || flags.alsologtostderr;
    let threshold = flags.stderrthreshold;
    let to_stderr = format::layer()
        .with_ansi(false)
        .with_writer(stderr)
        .with_filter(filter_fn(move |metadata| {
            every_line || Severity::of(metadata) >= threshold
        }));
    let (to_file, file_writer) = if flags.logtostderr {
        (None, None)
    } else {
        let dir = if flags.log_dir.is_empty() {
            default_log_dir
        } else {
            Path::new(&flags.log_dir)
        };
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(program)
            .filename_suffix("log")
            .build(dir)
            .map_err(|source| LogFileError {
                dir: dir.to_owned(),
                source,
            })?;
        let (writer, guard) = NonBlockingBuilder::default().lossy(false).finish(appender);
        (
            Some(format::layer().with_ansi(false).with_writer(writer)),
            Some(guard),
        )
    };
    let targets = Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target(PREBID_TARGETS, LevelFilter::INFO);
    let subscriber = Registry::default()
        .with(targets)
        .with(to_stderr)
        .with(to_file);
    Ok((
        Box::new(subscriber),
        Logging {
            _file_writer: file_writer,
        },
    ))
}

/// Starts Prebid Server's logger from its command line.
///
/// It parses `args` (`args[0]` being the program, as in `os.Args`) with [`parse_command_line`] and
/// installs the [`subscriber`] the flags ask for as the global default, which `log` records reach
/// too. Usage, errors and the lines go to `stderr`; `default_log_dir` is glog's `os.TempDir()`,
/// `TMPDIR` or `/tmp`.
///
/// # Errors
///
/// [`Exit::Help`] after `-h` or `-help`; [`Exit::Error`] when the command line does not parse,
/// the log file cannot be opened or a global subscriber is already set.
pub fn init_from_glog_flags<W>(
    args: &[OsString],
    default_log_dir: &Path,
    stderr: W,
) -> Result<Logging, Exit>
where
    W: for<'w> MakeWriter<'w> + Clone + Send + Sync + 'static,
{
    let command_line = parse_command_line(args, &mut stderr.make_writer())?;
    let program = args
        .first()
        .and_then(|program| Path::new(program).file_name())
        .map_or_else(
            || DEFAULT_PROGRAM.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
    let started = subscriber(
        &command_line.flags,
        &program,
        default_log_dir,
        stderr.clone(),
    )
    .map_err(|err| err.to_string())
    .and_then(|(global, logging)| {
        global
            .try_init()
            .map(|()| logging)
            .map_err(|err| err.to_string())
    });
    started.map_err(|err| {
        writeln!(stderr.make_writer(), "{err}").unwrap_or_default();
        Exit::Error
    })
}
