//! Glue (Rule B (c): CLI flags): Prebid Server's command line is glog v1.2.5's nine flags, which
//! `main.go:28` parses with `flag.Parse`. getopts tokenises them as Go's flag package does
//! (`long_only`, so `-name` is a long flag; `StopAtFirstFree`; `v` as getopts' one-letter name;
//! the bools as optional-value flags, so `-name` and `-name=false` work and `-name false` does
//! not), and this module gives the matches Go's meaning: each value is set in argument order with
//! the `Set` glog or the flag package gives the flag, so a bad value before a failing argument
//! decides the outcome; the last of repeated values wins; `-h` and `-help` print the usage and
//! exit 0; every other failure prints an error line and the usage and exits 2 (ledger:
//! `logging.flags`, restored). The error lines keep Go's words with values quoted as Rust quotes
//! strings and the usage listing is getopts' (ledger: `logging.flag-texts`); an argument that is
//! not UTF-8 fails (ledger: `logging.flag-utf8`).

mod strconv;

use std::{
    ffi::OsString,
    fmt,
    io::Write,
    process::{ExitCode, Termination},
};

use getopts::{Fail, HasArg, Matches, Occur, Options, ParsingStyle};

use crate::Severity;

/// glog's flags as the command line set them.
///
/// Each is read as glog's or the flag package's `Set` reads it. Only `-logtostderr`,
/// `-alsologtostderr`, `-stderrthreshold` and `-log_dir` change where lines go; the rest are
/// accepted for Go's command lines (ledger: `logging.v-flags`, `logging.files`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlogFlags {
    /// `-alsologtostderr`: log to standard error as well as files.
    pub alsologtostderr: bool,
    /// `-log_backtrace_at`: when logging hits line `file:N`, emit a stack trace.
    pub log_backtrace_at: Vec<TraceLocation>,
    /// `-log_dir`: if non-empty, write log files in this directory.
    pub log_dir: String,
    /// `-log_link`: if non-empty, add symbolic links in this directory to the log files.
    pub log_link: String,
    /// `-logbuflevel`: buffer log messages logged at this level or lower.
    pub logbuflevel: i64,
    /// `-logtostderr`: log to standard error instead of files.
    pub logtostderr: bool,
    /// `-stderrthreshold`: logs at or above this threshold go to stderr.
    pub stderrthreshold: Severity,
    /// `-v`: log level for V logs (Prebid Server has none).
    pub v: i32,
    /// `-vmodule`: comma-separated list of pattern=N settings for file-filtered logging.
    pub vmodule: Vec<ModulePattern>,
}

impl Default for GlogFlags {
    /// glog's defaults: `-stderrthreshold=ERROR` (`glog_flags.go:393`) and everything else zero.
    fn default() -> Self {
        Self {
            alsologtostderr: false,
            log_backtrace_at: Vec::new(),
            log_dir: String::new(),
            log_link: String::new(),
            logbuflevel: 0,
            logtostderr: false,
            stderrthreshold: Severity::Error,
            v: 0,
            vmodule: Vec::new(),
        }
    }
}

/// One `pattern=N` of `-vmodule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModulePattern {
    /// The file pattern.
    pub pattern: String,
    /// The level, cut to 32 bits as glog's `Level(v)` cuts it.
    pub level: i32,
}

impl fmt::Display for ModulePattern {
    /// glog's `vModuleFlag.String` form: `pattern=N`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.pattern, self.level)
    }
}

/// One `file.go:N` of `-log_backtrace_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceLocation {
    /// The file name, which glog only requires to contain a dot.
    pub file: String,
    /// The line, never negative.
    pub line: i64,
}

impl fmt::Display for TraceLocation {
    /// glog's `traceLocation.String` form: `file:N`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// A parsed command line: glog's flags and the arguments after them (`flag.Args()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandLine {
    /// The flags.
    pub flags: GlogFlags,
    /// The arguments left once the flags end.
    pub args: Vec<String>,
}

/// How `flag.Parse` ends the process (`flag.ExitOnError`) when the command line does not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// `-h` or `-help`: the usage listing was written; exit code 0.
    Help,
    /// A flag failed, or the logger could not start: an error line was written; exit code 2.
    Error,
}

impl Exit {
    /// The exit code: 0 or 2.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Help => 0,
            Self::Error => 2,
        }
    }
}

impl Termination for Exit {
    fn report(self) -> ExitCode {
        ExitCode::from(self.code())
    }
}

/// What one flag takes and how glog or the flag package sets it.
#[derive(Clone, Copy)]
enum Kind {
    /// A bool (`flag.BoolVar`): `-name` sets it, and `-name=value` sets `strconv.ParseBool`'s
    /// value.
    Bool(fn(&mut GlogFlags, bool)),
    /// A value, given after `=` or as the next argument; `hint` is what Go's usage calls it and
    /// `set` returns the reason a value fails.
    Value {
        hint: &'static str,
        set: fn(&mut GlogFlags, &str) -> Result<(), String>,
    },
}

/// One of glog's flags, with its usage text (glog_flags.go:388-397, glog_file.go:44-46).
struct Spec {
    name: &'static str,
    usage: &'static str,
    kind: Kind,
}

/// glog's flags, in Go's usage order.
const SPECS: [Spec; 9] = [
    Spec {
        name: "alsologtostderr",
        usage: "log to standard error as well as files",
        kind: Kind::Bool(|flags, on| flags.alsologtostderr = on),
    },
    Spec {
        name: "log_backtrace_at",
        usage: "when logging hits line file:N, emit a stack trace",
        kind: Kind::Value {
            hint: "value",
            set: set_log_backtrace_at,
        },
    },
    Spec {
        name: "log_dir",
        usage: "If non-empty, write log files in this directory",
        kind: Kind::Value {
            hint: "string",
            set: |flags, value| {
                value.clone_into(&mut flags.log_dir);
                Ok(())
            },
        },
    },
    Spec {
        name: "log_link",
        usage: "If non-empty, add symbolic links in this directory to the log files",
        kind: Kind::Value {
            hint: "string",
            set: |flags, value| {
                value.clone_into(&mut flags.log_link);
                Ok(())
            },
        },
    },
    Spec {
        name: "logbuflevel",
        usage: "Buffer log messages logged at this level or lower (-1 means don't buffer; 0 means \
                buffer INFO only; ...). Has limited applicability on non-prod platforms.",
        kind: Kind::Value {
            hint: "int",
            set: set_logbuflevel,
        },
    },
    Spec {
        name: "logtostderr",
        usage: "log to standard error instead of files",
        kind: Kind::Bool(|flags, on| flags.logtostderr = on),
    },
    Spec {
        name: "stderrthreshold",
        usage: "logs at or above this threshold go to stderr",
        kind: Kind::Value {
            hint: "value",
            set: set_stderrthreshold,
        },
    },
    Spec {
        name: "v",
        usage: "log level for V logs",
        kind: Kind::Value {
            hint: "value",
            set: |flags, value| {
                flags.v = strconv::to_int32(strconv::atoi(value).map_err(|err| err.to_string())?);
                Ok(())
            },
        },
    },
    Spec {
        name: "vmodule",
        usage: "comma-separated list of pattern=N settings for file-filtered logging",
        kind: Kind::Value {
            hint: "value",
            set: set_vmodule,
        },
    },
];

/// glog's `vModuleFlag.Set` (glog_flags.go:160-186).
fn set_vmodule(flags: &mut GlogFlags, value: &str) -> Result<(), String> {
    const SYNTAX: &str = "syntax error: expect comma-separated list of filename=N";
    let mut patterns = Vec::new();
    for pat in value.split(',').filter(|pat| !pat.is_empty()) {
        let mut parts = pat.split('=');
        let (Some(pattern), Some(level), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(SYNTAX.to_owned());
        };
        if pattern.is_empty() || level.is_empty() {
            return Err(SYNTAX.to_owned());
        }
        let Ok(level) = strconv::atoi(level) else {
            return Err(SYNTAX.to_owned());
        };
        patterns.push(ModulePattern {
            pattern: pattern.to_owned(),
            level: strconv::to_int32(level),
        });
    }
    flags.vmodule = patterns;
    Ok(())
}

/// glog's `traceLocations.Set` and `parseTraceLocation` (glog_flags.go:237-316).
fn set_log_backtrace_at(flags: &mut GlogFlags, value: &str) -> Result<(), String> {
    const SYNTAX: &str = "syntax error: expect file.go:234";
    let mut locations = Vec::new();
    for location in value.split(',').filter(|location| !location.is_empty()) {
        let mut fields = location.split(':');
        let (Some(file), Some(line), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err(SYNTAX.to_owned());
        };
        if !file.contains('.') {
            return Err(SYNTAX.to_owned());
        }
        let Ok(line) = strconv::atoi(line) else {
            return Err(SYNTAX.to_owned());
        };
        if line < 0 {
            return Err("negative value for line".to_owned());
        }
        locations.push(TraceLocation {
            file: file.to_owned(),
            line,
        });
    }
    flags.log_backtrace_at = locations;
    Ok(())
}

/// glog's `severityFlag.Set` (glog_flags.go:341-356): a severity's name in any case
/// (`strings.ToUpper`, which agrees with Rust's upper-casing on every string that turns into one
/// of the four names), else a number cut to `logsink.Severity`'s eight bits.
fn set_stderrthreshold(flags: &mut GlogFlags, value: &str) -> Result<(), String> {
    let upper = value.to_uppercase();
    flags.stderrthreshold = if let Some(&severity) = Severity::ALL
        .iter()
        .find(|severity| severity.name() == upper)
    {
        severity
    } else {
        let v = strconv::atoi(value).map_err(|err| err.to_string())?;
        Severity::from_number(strconv::to_int8(v))
            .ok_or_else(|| format!("Severity {v} out of range (min 0, max 3)."))?
    };
    Ok(())
}

/// The flag package's `intValue.Set` (flag/flag.go:163-170): `strconv.ParseInt(s, 0, 64)`, its
/// errors as `numError` words them.
fn set_logbuflevel(flags: &mut GlogFlags, value: &str) -> Result<(), String> {
    flags.logbuflevel = strconv::parse_int(value, true).map_err(|kind| {
        match kind {
            strconv::NumErrorKind::Syntax => "parse error",
            strconv::NumErrorKind::Range => "value out of range",
        }
        .to_owned()
    })?;
    Ok(())
}

/// How parsing stopped before the flags ended.
enum Failure {
    /// `-h` or `-help` (Go: `flag.ErrHelp`).
    Help,
    /// The line Go's `failf` writes before the usage listing.
    Error(String),
}

/// getopts' options for glog's flags.
fn options() -> Options {
    let mut options = Options::new();
    let _ = options
        .long_only(true)
        .parsing_style(ParsingStyle::StopAtFirstFree);
    for spec in &SPECS {
        // getopts takes a one-letter name only as a short one; `-v` and `--v` both reach it.
        let (short, long) = if spec.name.len() == 1 {
            (spec.name, "")
        } else {
            ("", spec.name)
        };
        let (hint, has_arg) = match spec.kind {
            Kind::Bool(_) => ("=bool", HasArg::Maybe),
            Kind::Value { hint, .. } => (hint, HasArg::Yes),
        };
        let _ = options.opt(short, long, spec.usage, hint, has_arg, Occur::Multi);
    }
    options
}

/// Parses Prebid Server's command line as `flag.Parse` does.
///
/// `args[0]` is the program, as in `os.Args`. On `-h` or `-help` it writes the usage listing to
/// `stderr` and returns [`Exit::Help`]; on any other failure it writes the error line and the
/// usage listing and returns [`Exit::Error`]. A failed write is dropped, as the flag package drops
/// it.
///
/// # Errors
///
/// [`Exit`] when the command line asks for the usage or does not parse.
pub fn parse_command_line(args: &[OsString], stderr: &mut dyn Write) -> Result<CommandLine, Exit> {
    let (program, args) = args
        .split_first()
        .map_or((String::new(), &[][..]), |(program, args)| {
            (program.to_string_lossy().into_owned(), args)
        });
    let usage = options().usage(&format!("Usage of {program}:"));
    let (text, exit) = match parse(args) {
        Ok(command_line) => return Ok(command_line),
        Err(Failure::Help) => (usage, Exit::Help),
        Err(Failure::Error(line)) => (format!("{line}\n{usage}"), Exit::Error),
    };
    stderr.write_all(text.as_bytes()).unwrap_or_default();
    Err(exit)
}

/// The flags of `args`, the program left out.
fn parse(args: &[OsString]) -> Result<CommandLine, Failure> {
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .ok_or_else(|| Failure::Error(format!("argument is not UTF-8: {}", arg.display())))
        })
        .collect::<Result<Vec<&str>, Failure>>()?;
    let options = options();
    let fail = match options.parse(&args) {
        Ok(matches) => return set_in_order(&matches),
        Err(fail) => fail,
    };
    // getopts reports a failure without the matches before it, while Go sets each value as it
    // reads it: the longest prefix getopts parses ends just before the failing argument, and a
    // bad value in it fails first.
    let parses = |end: usize| {
        args.get(..end)
            .is_some_and(|prefix| options.parse(prefix).is_ok())
    };
    let at = (0..args.len()).rev().find(|&end| parses(end)).unwrap_or(0);
    if let Ok(matches) = options.parse(args.get(..at).unwrap_or_default())
        && let Err(failure) = set_in_order(&matches)
    {
        return Err(failure);
    }
    Err(failure_at(args.get(at).copied().unwrap_or_default(), &fail))
}

/// Sets the matched values in argument order, as `flag.Parse` sets each flag as it reads it.
fn set_in_order(matches: &Matches) -> Result<CommandLine, Failure> {
    let mut sets = Vec::new();
    for spec in &SPECS {
        let values = matches.opt_strs_pos(spec.name);
        for at in matches.opt_positions(spec.name) {
            let value = values
                .iter()
                .find(|(position, _)| *position == at)
                .map(|(_, value)| value.clone());
            sets.push((at, spec, value));
        }
    }
    sets.sort_by_key(|&(at, ..)| at);
    let mut flags = GlogFlags::default();
    for (_, spec, value) in sets {
        match (spec.kind, value.as_deref()) {
            (Kind::Bool(set), None) => set(&mut flags, true),
            (Kind::Bool(set), Some(value)) => match strconv::parse_bool(value) {
                Some(on) => set(&mut flags, on),
                None => {
                    return Err(Failure::Error(format!(
                        "invalid boolean value {value:?} for -{}: parse error",
                        spec.name
                    )));
                }
            },
            (Kind::Value { set, .. }, Some(value)) => set(&mut flags, value).map_err(|reason| {
                Failure::Error(format!(
                    "invalid value {value:?} for flag -{}: {reason}",
                    spec.name
                ))
            })?,
            (Kind::Value { .. }, None) => {
                return Err(Failure::Error(format!(
                    "flag needs an argument: -{}",
                    spec.name
                )));
            }
        }
    }
    Ok(CommandLine {
        flags,
        args: matches.free.clone(),
    })
}

/// What Go's `parseOne` (flag/flag.go:1075-1116) says of the argument getopts failed at.
fn failure_at(arg: &str, fail: &Fail) -> Failure {
    let name = arg
        .strip_prefix("--")
        .or_else(|| arg.strip_prefix('-'))
        .unwrap_or(arg);
    if name.is_empty() || name.starts_with(['-', '=']) {
        return Failure::Error(format!("bad flag syntax: {arg}"));
    }
    // Go looks for `=` from the name's second byte on.
    let name = name
        .bytes()
        .skip(1)
        .position(|b| b == b'=')
        .and_then(|at| name.get(..=at))
        .unwrap_or(name);
    match fail {
        Fail::ArgumentMissing(_) => Failure::Error(format!("flag needs an argument: -{name}")),
        Fail::UnrecognizedOption(_)
        | Fail::OptionMissing(_)
        | Fail::OptionDuplicated(_)
        | Fail::UnexpectedArgument(_) => {
            if name == "h" || name == "help" {
                Failure::Help
            } else {
                Failure::Error(format!("flag provided but not defined: -{name}"))
            }
        }
    }
}
