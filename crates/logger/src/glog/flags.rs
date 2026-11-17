//! Go: `glog_flags.go` and the `-log_dir`, `-log_link` and `-logbuflevel` flags of
//! `glog_file.go`, parsed by Go's `flag` package (`FlagSet.Parse`, `Set` and `PrintDefaults`).

use std::{error, fmt};

use pbs_gocompat::strconv::{self, NumErrorKind};

use super::{Severity, Site, parse_severity};

/// A `-vmodule` entry (Go: `modulePat`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModulePattern {
    /// The file pattern.
    pub pattern: String,
    /// Whether the pattern has no `\*?[]` metacharacters.
    pub literal: bool,
    /// Whether the pattern has a `/` and so matches the full path.
    pub full: bool,
    /// The verbosity level.
    pub level: i32,
}

/// A `-log_backtrace_at` entry (Go: `traceLocation`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceLocation {
    /// The file's base name, with its extension.
    pub file: String,
    /// The line.
    pub line: i64,
}

impl TraceLocation {
    /// Go: `traceLocation.match`.
    fn matches(&self, site: Site<'_>) -> bool {
        self.line == i64::from(site.line) && self.file == site.base_name()
    }
}

/// glog's command-line flags (Go: the package variables they set).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flags {
    /// `-v`: the verbosity level.
    pub v: i32,
    /// `-vmodule`: per-file verbosity levels.
    pub vmodule: Vec<ModulePattern>,
    /// `-log_backtrace_at`: call sites that also log a stack trace.
    pub log_backtrace_at: Vec<TraceLocation>,
    /// `-logtostderr`: log to standard error instead of files.
    pub logtostderr: bool,
    /// `-alsologtostderr`: log to standard error as well as files.
    pub alsologtostderr: bool,
    /// `-stderrthreshold`: the lowest severity also written to standard error.
    pub stderrthreshold: Severity,
    /// `-log_dir`: the log file directory; empty means the temporary directory.
    pub log_dir: String,
    /// `-log_link`: a directory for further links to the log files.
    pub log_link: String,
    /// `-logbuflevel`: lines above this severity trigger a flush of the log files.
    pub logbuflevel: i64,
}

impl Default for Flags {
    /// glog's defaults: `-stderrthreshold=ERROR` and everything else zero.
    fn default() -> Self {
        Self {
            v: 0,
            vmodule: Vec::new(),
            log_backtrace_at: Vec::new(),
            logtostderr: false,
            alsologtostderr: false,
            stderrthreshold: Severity::Error,
            log_dir: String::new(),
            log_link: String::new(),
            logbuflevel: 0,
        }
    }
}

/// How a flag's value parses and prints (Go: its `flag.Value` type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Go: `glog.Level`.
    Level,
    /// Go: `vModuleFlag`.
    VModule,
    /// Go: `traceLocations`.
    Trace,
    /// Go: `flag.BoolVar`.
    Bool,
    /// Go: `severityFlag`.
    Severity,
    /// Go: `flag.String`.
    Str,
    /// Go: `flag.Int`.
    Int,
}

/// A registered flag (Go: `flag.Flag`).
#[derive(Clone, Copy, Debug)]
struct Def {
    name: &'static str,
    kind: Kind,
    usage: &'static str,
    /// Go: `DefValue`.
    default: &'static str,
}

/// glog's flags in Go's `VisitAll` (sorted) order.
const DEFS: [Def; 9] = [
    Def {
        name: "alsologtostderr",
        kind: Kind::Bool,
        usage: "log to standard error as well as files",
        default: "false",
    },
    Def {
        name: "log_backtrace_at",
        kind: Kind::Trace,
        usage: "when logging hits line file:N, emit a stack trace",
        default: "",
    },
    Def {
        name: "log_dir",
        kind: Kind::Str,
        usage: "If non-empty, write log files in this directory",
        default: "",
    },
    Def {
        name: "log_link",
        kind: Kind::Str,
        usage: "If non-empty, add symbolic links in this directory to the log files",
        default: "",
    },
    Def {
        name: "logbuflevel",
        kind: Kind::Int,
        usage: "Buffer log messages logged at this level or lower (-1 means don't buffer; 0 \
                means buffer INFO only; ...). Has limited applicability on non-prod platforms.",
        default: "0",
    },
    Def {
        name: "logtostderr",
        kind: Kind::Bool,
        usage: "log to standard error instead of files",
        default: "false",
    },
    Def {
        name: "stderrthreshold",
        kind: Kind::Severity,
        usage: "logs at or above this threshold go to stderr",
        default: "2",
    },
    Def {
        name: "v",
        kind: Kind::Level,
        usage: "log level for V logs",
        default: "0",
    },
    Def {
        name: "vmodule",
        kind: Kind::VModule,
        usage: "comma-separated list of pattern=N settings for file-filtered logging",
        default: "",
    },
];

impl Kind {
    /// Go: the name `flag.UnquoteUsage` gives the value in usage text.
    const fn value_name(self) -> &'static str {
        match self {
            Self::Bool => "",
            Self::Int => "int",
            Self::Str => "string",
            Self::Level | Self::VModule | Self::Trace | Self::Severity => "value",
        }
    }

    /// Go: the `String()` of the type's zero value, which `isZeroValue` compares.
    const fn zero(self) -> &'static str {
        match self {
            Self::Bool => "false",
            Self::Int | Self::Level | Self::Severity => "0",
            Self::Str | Self::VModule | Self::Trace => "",
        }
    }
}

/// The error of [`Flags::set`]: Go's `flag.Value.Set` error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetError(String);

impl fmt::Display for SetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl error::Error for SetError {}

/// The error of [`Flags::parse`]: help was requested, or a flag was undefined, malformed or
/// given a bad value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagError {
    /// The error text, or `None` for `-h` and `-help` (Go: `flag.ErrHelp`).
    message: Option<String>,
}

impl FlagError {
    /// Whether this is `-h` or `-help` (Go: `flag.ErrHelp`).
    #[must_use]
    pub const fn is_help(&self) -> bool {
        self.message.is_none()
    }

    /// What Go's `flag.Parse` prints to standard error: the error line, if any, then the usage
    /// of `program` (Go: `os.Args[0]`).
    #[must_use]
    pub fn output(&self, program: &str) -> String {
        let usage = usage(program);
        match &self.message {
            Some(message) => format!("{message}\n{usage}"),
            None => usage,
        }
    }

    /// The status Go's `flag.Parse` exits with: 0 for help, else 2.
    #[must_use]
    pub const fn exit_status(&self) -> i32 {
        if self.is_help() { 0 } else { 2 }
    }
}

impl fmt::Display for FlagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message.as_deref().unwrap_or("flag: help requested"))
    }
}

impl error::Error for FlagError {}

/// Go: `int32(v)`, keeping the low 32 bits.
const fn low_i32(v: i64) -> i32 {
    let [b0, b1, b2, b3, ..] = v.to_le_bytes();
    i32::from_le_bytes([b0, b1, b2, b3])
}

/// Go: `int8(v)`, keeping the low 8 bits.
const fn low_i8(v: i64) -> i8 {
    let [b0, ..] = v.to_le_bytes();
    i8::from_le_bytes([b0])
}

/// Go: `errVmoduleSyntax`.
const VMODULE_SYNTAX: &str = "syntax error: expect comma-separated list of filename=N";

/// Go: `errTraceSyntax`.
const TRACE_SYNTAX: &str = "syntax error: expect file.go:234";

/// Go: `vModuleFlag.Set`.
fn parse_vmodule(value: &str) -> Result<Vec<ModulePattern>, SetError> {
    let mut filter = Vec::new();
    for pat in value.split(',').filter(|p| !p.is_empty()) {
        let parts: Vec<&str> = pat.split('=').collect();
        let [pattern, level] = parts.as_slice() else {
            return Err(SetError(VMODULE_SYNTAX.to_owned()));
        };
        if pattern.is_empty() || level.is_empty() {
            return Err(SetError(VMODULE_SYNTAX.to_owned()));
        }
        let level =
            strconv::atoi(level).map_err(|_atoi_error| SetError(VMODULE_SYNTAX.to_owned()))?;
        filter.push(ModulePattern {
            pattern: (*pattern).to_owned(),
            literal: !pattern.contains(['\\', '*', '?', '[', ']']),
            full: pattern.contains('/'),
            level: low_i32(level),
        });
    }
    Ok(filter)
}

/// Go: `parseTraceLocation`.
fn parse_trace_location(value: &str) -> Result<TraceLocation, SetError> {
    let syntax = || SetError(TRACE_SYNTAX.to_owned());
    let parts: Vec<&str> = value.split(':').collect();
    let [file, line] = parts.as_slice() else {
        return Err(syntax());
    };
    if !file.contains('.') {
        return Err(syntax());
    }
    let line = strconv::atoi(line).map_err(|_atoi_error| syntax())?;
    if line < 0 {
        return Err(SetError("negative value for line".to_owned()));
    }
    Ok(TraceLocation {
        file: (*file).to_owned(),
        line,
    })
}

/// Go: `traceLocations.Set`.
fn parse_trace_locations(value: &str) -> Result<Vec<TraceLocation>, SetError> {
    value
        .split(',')
        .filter(|s| !s.is_empty())
        .map(parse_trace_location)
        .collect()
}

/// Go: `severityFlag.Set`: a severity name, or a number whose low byte is 0 to 3.
fn parse_threshold(value: &str) -> Result<Severity, SetError> {
    if let Ok(severity) = parse_severity(value) {
        return Ok(severity);
    }
    let v = strconv::atoi(value).map_err(|e| SetError(e.to_string()))?;
    Severity::from_int(low_i8(v))
        .ok_or_else(|| SetError(format!("Severity {v} out of range (min 0, max 3).")))
}

/// Go: `flag.intValue.Set`: `strconv.ParseInt(s, 0, 64)` with Go's flag error texts.
///
/// Like Go, the flag takes the parsed value even with an error: 0, or the value clamped to the
/// `int64` range.
fn parse_int_flag(value: &str) -> (i64, Option<SetError>) {
    let (v, err) = strconv::parse_int_pair(value, 0, 64);
    let err = err.map(|e| {
        SetError(match e.err {
            NumErrorKind::Syntax => "parse error".to_owned(),
            NumErrorKind::Range => "value out of range".to_owned(),
            NumErrorKind::Base(_) | NumErrorKind::BitSize(_) => e.to_string(),
        })
    });
    (v, err)
}

/// Go: `flag.boolValue.Set`; like Go, the flag becomes false when the value does not parse.
fn parse_bool_flag(value: &str) -> (bool, Option<SetError>) {
    strconv::parse_bool(value).map_or_else(
        |_parse_error| (false, Some(SetError("parse error".to_owned()))),
        |v| (v, None),
    )
}

/// Stores a value that Go's `Set` stores even when it reports an error.
fn store<T>(slot: &mut T, (value, err): (T, Option<SetError>)) -> Result<(), SetError> {
    *slot = value;
    err.map_or(Ok(()), Err)
}

impl Flags {
    /// Whether a line at `site` also logs a stack trace (Go: `backtraceAt`).
    pub(super) fn backtrace_at(&self, site: Site<'_>) -> bool {
        self.log_backtrace_at.iter().any(|t| t.matches(site))
    }

    /// Go: `flag.Set(name, value)`.
    ///
    /// # Errors
    ///
    /// `no such flag -name`, or the flag's parse error. As in Go, a bool flag then becomes
    /// false and `logbuflevel` takes the value Go's `ParseInt` returns with the error; the
    /// other flags keep their value.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), SetError> {
        let def = DEFS
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| SetError(format!("no such flag -{name}")))?;
        self.set_def(def, value)
    }

    fn set_def(&mut self, def: &Def, value: &str) -> Result<(), SetError> {
        match def.name {
            "alsologtostderr" => store(&mut self.alsologtostderr, parse_bool_flag(value))?,
            "log_backtrace_at" => self.log_backtrace_at = parse_trace_locations(value)?,
            "log_dir" => value.clone_into(&mut self.log_dir),
            "log_link" => value.clone_into(&mut self.log_link),
            "logbuflevel" => store(&mut self.logbuflevel, parse_int_flag(value))?,
            "logtostderr" => store(&mut self.logtostderr, parse_bool_flag(value))?,
            "stderrthreshold" => self.stderrthreshold = parse_threshold(value)?,
            "v" => {
                self.v = low_i32(strconv::atoi(value).map_err(|e| SetError(e.to_string()))?);
            }
            "vmodule" => self.vmodule = parse_vmodule(value)?,
            _ => return Err(SetError(format!("no such flag -{}", def.name))),
        }
        Ok(())
    }

    /// Go: `flag.Lookup(name).Value.String()`.
    #[must_use]
    pub fn value_string(&self, name: &str) -> Option<String> {
        let join = |items: Vec<String>| items.join(",");
        Some(match name {
            "alsologtostderr" => strconv::format_bool(self.alsologtostderr).to_owned(),
            "log_backtrace_at" => join(
                self.log_backtrace_at
                    .iter()
                    .map(|t| format!("{}:{}", t.file, t.line))
                    .collect(),
            ),
            "log_dir" => self.log_dir.clone(),
            "log_link" => self.log_link.clone(),
            "logbuflevel" => self.logbuflevel.to_string(),
            "logtostderr" => strconv::format_bool(self.logtostderr).to_owned(),
            "stderrthreshold" => self.stderrthreshold.as_int().to_string(),
            "v" => self.v.to_string(),
            "vmodule" => join(
                self.vmodule
                    .iter()
                    .map(|m| format!("{}={}", m.pattern, m.level))
                    .collect(),
            ),
            _ => return None,
        })
    }

    /// Go: `FlagSet.Parse(args)` over glog's flags: flags until the first non-flag argument or
    /// `--`; returns the arguments after them.
    ///
    /// # Errors
    ///
    /// As Go's `flag` package: help for `-h` or `-help`, else `bad flag syntax: ...`,
    /// `flag provided but not defined: -name`, `flag needs an argument: -name` or
    /// `invalid value "x" for flag -name: ...`.
    pub fn parse(&mut self, args: &[String]) -> Result<Vec<String>, FlagError> {
        let mut rest = args;
        while let Some((s, after)) = rest.split_first() {
            let bytes = s.as_bytes();
            if bytes.len() < 2 || bytes.first() != Some(&b'-') {
                break;
            }
            let minuses = if bytes.get(1) == Some(&b'-') {
                if bytes.len() == 2 {
                    // "--" terminates the flags.
                    rest = after;
                    break;
                }
                2
            } else {
                1
            };
            let name = s.get(minuses..).unwrap_or_default();
            if name.is_empty() || name.starts_with(['-', '=']) {
                return Err(fail(format!("bad flag syntax: {s}")));
            }
            rest = after;
            let (name, value) = name
                .split_once('=')
                .map_or((name, None), |(n, v)| (n, Some(v)));
            let Some(def) = DEFS.iter().find(|d| d.name == name) else {
                if name == "help" || name == "h" {
                    return Err(FlagError { message: None });
                }
                return Err(fail(format!("flag provided but not defined: -{name}")));
            };
            if def.kind == Kind::Bool {
                let value = value.unwrap_or("true");
                self.set_def(def, value).map_err(|e| {
                    fail(format!(
                        "invalid boolean value {} for -{name}: {e}",
                        strconv::quote(value)
                    ))
                })?;
                continue;
            }
            let value = if let Some(v) = value {
                v
            } else {
                // The value is the next argument.
                let Some((next, after_value)) = rest.split_first() else {
                    return Err(fail(format!("flag needs an argument: -{name}")));
                };
                rest = after_value;
                next.as_str()
            };
            self.set_def(def, value).map_err(|e| {
                fail(format!(
                    "invalid value {} for flag -{name}: {e}",
                    strconv::quote(value)
                ))
            })?;
        }
        Ok(rest.to_vec())
    }
}

/// A parse error with its message.
const fn fail(message: String) -> FlagError {
    FlagError {
        message: Some(message),
    }
}

/// Go: `flag.PrintDefaults()` for glog's flags.
#[must_use]
pub fn print_defaults() -> String {
    let mut out = String::new();
    for def in &DEFS {
        let mut b = format!("  -{}", def.name);
        let value_name = def.kind.value_name();
        if !value_name.is_empty() {
            b.push(' ');
            b.push_str(value_name);
        }
        // Go puts the usage of a one-letter boolean flag on the same line.
        if b.len() <= 4 {
            b.push('\t');
        } else {
            b.push_str("\n    \t");
        }
        b.push_str(&def.usage.replace('\n', "\n    \t"));
        if def.default != def.kind.zero() {
            let shown = if def.kind == Kind::Str {
                strconv::quote(def.default)
            } else {
                def.default.to_owned()
            };
            b.push_str(" (default ");
            b.push_str(&shown);
            b.push(')');
        }
        out.push_str(&b);
        out.push('\n');
    }
    out
}

/// Go: the default `flag.Usage` of a program: `Usage of program:` and the flag defaults.
#[must_use]
pub fn usage(program: &str) -> String {
    format!("Usage of {program}:\n{}", print_defaults())
}
