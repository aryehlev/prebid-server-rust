//! glog tests: Go's flag parsing, line format and log files replayed from goldens, and the
//! fatal paths in child processes.

use std::{
    env,
    error::Error,
    fs, io,
    ops::Range,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    thread,
    time::Duration as StdDuration,
};

use pbs_gocompat::{
    fmt::Value,
    time::{DateFields, Location, Time},
};

use super::{
    Env, Flags, Glog, Severity, Site,
    format::{self, MAX_LOG_MESSAGE_LEN},
    parse_severity,
};
use crate::testutil::{
    SharedBuf, TempDir, TestResult, check, check_eq, fields, fixed_time, read_golden,
};

/// glog's flag names in Go's `VisitAll` order.
const FLAG_NAMES: [&str; 9] = [
    "alsologtostderr",
    "log_backtrace_at",
    "log_dir",
    "log_link",
    "logbuflevel",
    "logtostderr",
    "stderrthreshold",
    "v",
    "vmodule",
];

/// Every flag as `name=value` lines, as the generator prints them.
fn flag_values(flags: &Flags) -> String {
    FLAG_NAMES
        .iter()
        .map(|name| format!("{name}={}", flags.value_string(name).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_lines(s: &str) -> Vec<String> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split('\n').map(str::to_owned).collect()
    }
}

#[test]
fn flags_golden() -> TestResult {
    // Go's FlagSet.Parse over glog's flags: values, errors, printed usage and the arguments
    // left after the flags.
    let rows = read_golden("flags.txt")?;
    check(rows.len() >= 60, "flags.txt is complete")?;
    for row in &rows {
        let [args, want_err, want_out, want_rest, want_values] = fields::<5>(row)?;
        let mut flags = Flags::default();
        let result = flags.parse(&split_lines(args));
        match result {
            Ok(rest) => {
                check_eq(want_err, "", args)?;
                check_eq(rest.join("\n").as_str(), want_rest, args)?;
            }
            Err(err) => {
                let text = if err.is_help() {
                    "help".to_owned()
                } else {
                    err.to_string()
                };
                check_eq(text.as_str(), want_err, args)?;
                check_eq(err.output("prebid-server").as_str(), want_out, args)?;
                check_eq(&err.exit_status(), &if err.is_help() { 0 } else { 2 }, args)?;
            }
        }
        check_eq(flag_values(&flags).as_str(), want_values, args)?;
    }
    Ok(())
}

#[test]
fn flagset_golden() -> TestResult {
    // Go's FlagSet.Set: the Value.Set error texts, and the flag unchanged on error.
    for row in &read_golden("flagset.txt")? {
        let [name, value, want_err, want_values] = fields::<4>(row)?;
        let mut flags = Flags::default();
        let got = flags.set(name, value).err().map(|e| e.to_string());
        check_eq(got.unwrap_or_default().as_str(), want_err, name)?;
        check_eq(flag_values(&flags).as_str(), want_values, name)?;
    }
    Ok(())
}

#[test]
fn parse_severity_matches_go() -> TestResult {
    // Go: logsink.ParseSeverity upper-cases with strings.ToUpper.
    let cases = [
        ("INFO", Ok(Severity::Info)),
        ("warning", Ok(Severity::Warning)),
        ("Error", Ok(Severity::Error)),
        ("fatal", Ok(Severity::Fatal)),
        ("\u{131}nfo", Ok(Severity::Info)),
        ("warn", Err(r#"logsink: invalid severity "WARN""#)),
        ("", Err(r#"logsink: invalid severity """#)),
        (
            "\u{130}NFO",
            Err("logsink: invalid severity \"\u{130}NFO\""),
        ),
    ];
    for (name, want) in cases {
        let got = parse_severity(name).map_err(|e| e.to_string());
        check_eq(&got, &want.map_err(str::to_owned), name)?;
    }
    for s in Severity::ALL {
        check_eq(&Severity::from_int(s.as_int()), &Some(s), s.as_str())?;
    }
    check_eq(&Severity::from_int(4), &None, "4")
}

/// Decodes a golden argument (`s:`, `d:`, `f:`, `b:` or `nil`).
fn decode_arg(arg: &str) -> Result<Value, Box<dyn Error>> {
    if arg == "nil" {
        return Ok(Value::Nil);
    }
    let (kind, v) = arg.split_once(':').ok_or("bad argument")?;
    Ok(match kind {
        "s" => Value::from(v),
        "d" => Value::Int(v.parse()?),
        "f" => Value::Float64(v.parse()?),
        "b" => Value::Bool(v.parse()?),
        _ => return Err(format!("bad argument {arg}").into()),
    })
}

/// The time and site of a Go glog line's header (the year is not printed).
fn parse_header(line: &str) -> Result<(Time, String, u32), Box<dyn Error>> {
    let num = |range: Range<usize>| -> Result<i64, Box<dyn Error>> {
        Ok(line.get(range).ok_or("short line")?.parse()?)
    };
    let t = Time::date(
        &DateFields {
            year: 2026,
            month: num(1..3)?,
            day: num(3..5)?,
            hour: num(6..8)?,
            minute: num(9..11)?,
            second: num(12..14)?,
            nanosecond: num(15..21)? * 1000,
        },
        Location::Utc,
    );
    let rest = line.get(30..).ok_or("short line")?;
    let site = rest.split_once("] ").ok_or("no site")?.0;
    let (file, n) = site.rsplit_once(':').ok_or("no line")?;
    Ok((t, file.to_owned(), n.parse()?))
}

#[test]
fn lines_golden() -> TestResult {
    // glog's exact stderr bytes: severity letters, the pid column, Go's fmt verbs and errors,
    // Infoln and Info operand spacing, newline handling and the 15000-byte cut.
    let rows = read_golden("lines.txt")?;
    check(rows.len() >= 25, "lines.txt is complete")?;
    for row in &rows {
        let [name, sev, func, format, pid, want] = fields::<6>(row)?;
        let args = row
            .get(6..)
            .unwrap_or_default()
            .iter()
            .map(|a| decode_arg(a))
            .collect::<Result<Vec<Value>, _>>()?;
        let severity = parse_severity(sev)?;
        let format = match func {
            "depthf" => format.to_owned(),
            "ln" => format::ln_format(&args),
            "print" => format::default_format(&args),
            _ => return Err(format!("bad function {func}").into()),
        };
        let (t, file, line) = parse_header(want)?;
        let site = Site { file: &file, line };
        let got = format::line(severity, &t, pid.parse()?, site, &format, &args);
        check_eq(String::from_utf8(got)?.as_str(), want, name)?;
        check(want.len() <= MAX_LOG_MESSAGE_LEN, name)?;
    }
    Ok(())
}

#[test]
fn header_pads_like_go() -> TestResult {
    let mut buf = Vec::new();
    let site = Site {
        file: "/a/b/c.go",
        line: 9,
    };
    format::header(&mut buf, Severity::Warning, &fixed_time(), 4242, site);
    check_eq(
        String::from_utf8(buf)?.as_str(),
        "W0123 01:23:23.123123    4242 c.go:9] ",
        "header",
    )?;
    let mut wide_buf = Vec::new();
    let midnight = Time::from_unix(0, 5000);
    let wide = Site {
        file: "noslash.rs",
        line: 123_456,
    };
    format::header(&mut wide_buf, Severity::Fatal, &midnight, 123_456_789, wide);
    check_eq(
        String::from_utf8(wide_buf)?.as_str(),
        "F0101 00:00:00.000005 123456789 noslash.rs:123456] ",
        "a pid wider than seven columns",
    )
}

fn test_env(temp_dir: &Path) -> Env {
    Env {
        program: "golden".to_owned(),
        host: "HOST".to_owned(),
        user: "USER".to_owned(),
        pid: 4242,
        temp_dir: temp_dir.to_path_buf(),
        clock: fixed_time,
    }
}

/// A glog whose standard error is a buffer.
fn buffered_glog(env: Env, flags: Flags) -> (Arc<Glog>, SharedBuf) {
    let stderr = SharedBuf::default();
    (Glog::new(env, flags, Box::new(stderr.clone())), stderr)
}

const HERE: Site<'static> = Site {
    file: "/src/here.rs",
    line: 7,
};

#[test]
fn stderr_threshold_and_logtostderr() -> TestResult {
    let dir = TempDir::new("glog-threshold")?;
    let flags = Flags {
        log_dir: dir.path().display().to_string(),
        ..Flags::default()
    };
    let (glog, stderr) = buffered_glog(test_env(dir.path()), flags);
    glog.logf(HERE, Severity::Info, "quiet", &[]);
    glog.logf(HERE, Severity::Warning, "quiet too", &[]);
    glog.logf(HERE, Severity::Error, "loud %d", &[Value::Int(1)]);
    check_eq(
        stderr.take().as_str(),
        "E0123 01:23:23.123123    4242 here.rs:7] loud 1\n",
        "ERROR and above reach stderr by default",
    )?;
    glog.set_flag("alsologtostderr", "true")?;
    glog.logf(HERE, Severity::Info, "both", &[]);
    check_eq(
        stderr.take().as_str(),
        "I0123 01:23:23.123123    4242 here.rs:7] both\n",
        "alsologtostderr",
    )?;
    glog.flush();
    let info = fs::read_to_string(dir.path().join("golden.INFO"))?;
    let info_tail = concat!(
        "] quiet\n",
        "W0123 01:23:23.123123    4242 here.rs:7] quiet too\n",
        "E0123 01:23:23.123123    4242 here.rs:7] loud 1\n",
        "I0123 01:23:23.123123    4242 here.rs:7] both\n",
    );
    check(info.ends_with(info_tail), "INFO file")?;
    let (to_stderr, only_stderr) = buffered_glog(
        test_env(dir.path()),
        Flags {
            logtostderr: true,
            stderrthreshold: Severity::Fatal,
            ..Flags::default()
        },
    );
    to_stderr.logf(HERE, Severity::Info, "only stderr", &[]);
    check_eq(
        only_stderr.take().as_str(),
        "I0123 01:23:23.123123    4242 here.rs:7] only stderr\n",
        "logtostderr",
    )
}

#[test]
fn backtrace_at_a_site() -> TestResult {
    let flags = Flags {
        logtostderr: true,
        ..Flags::default()
    };
    let (glog, stderr) = buffered_glog(test_env(&env::temp_dir()), flags);
    glog.set_flag("log_backtrace_at", "here.rs:7")?;
    glog.logf(HERE, Severity::Info, "traced %s", &[Value::from("x")]);
    let other = Site { line: 8, ..HERE };
    glog.logf(other, Severity::Info, "plain", &[]);
    let out = stderr.take();
    check(
        out.starts_with("I0123 01:23:23.123123    4242 here.rs:7] traced x\n\n"),
        &out,
    )?;
    check(
        out.ends_with("\n\nI0123 01:23:23.123123    4242 here.rs:8] plain\n"),
        &out,
    )
}

/// Go generator's normalization of a log file name.
fn normalize_name(name: &str) -> String {
    let mut parts: Vec<&str> = name.split('.').collect();
    if let [_, host, user, _, _, time, pid] = parts.as_mut_slice() {
        *host = "HOST";
        *user = "USER";
        *time = "TIME";
        *pid = "PID";
    }
    parts.join(".")
}

/// Go generator's normalization of a path under the log directory.
fn normalize_path(p: &str, dir: &str) -> String {
    if let Some(rel) = p.strip_prefix(&format!("{dir}/")) {
        return format!("DIR/{}", normalize_name(rel));
    }
    if p.contains('/') {
        p.to_owned()
    } else {
        normalize_name(p)
    }
}

/// Go generator's normalization of a log file's contents.
fn normalize_content(content: &str, dir: &str) -> String {
    content
        .split_inclusive('\n')
        .map(|l| {
            if l.starts_with("Log file created at: ") {
                "Log file created at: TIME\n".to_owned()
            } else if l.starts_with("Running on machine: ") {
                "Running on machine: HOST\n".to_owned()
            } else if l.starts_with("Binary: ") {
                "Binary: BINARY\n".to_owned()
            } else if let Some((head, path)) = ["Previous log: ", "Next log: "]
                .iter()
                .find_map(|p| l.strip_prefix(p).map(|rest| (*p, rest)))
            {
                format!(
                    "{head}{}\n",
                    normalize_path(path.trim_end_matches('\n'), dir)
                )
            } else if l.starts_with("Log line format: ") {
                l.to_owned()
            } else if l.len() > 30
                && let Some(i) = l.find("] ")
            {
                format!(
                    "{} SITE{}",
                    l.get(..1).unwrap_or_default(),
                    l.get(i..).unwrap_or_default()
                )
            } else {
                l.to_owned()
            }
        })
        .collect()
}

/// The generator's rows for a directory of glog files and links.
fn list_dir(kind: &str, dir: &Path, log_dir: &str) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    let mut rows = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = fs::symlink_metadata(entry.path())?;
        let row = if meta.file_type().is_symlink() {
            let target = fs::read_link(entry.path())?.display().to_string();
            vec![
                kind.to_owned(),
                normalize_name(&name),
                "link".to_owned(),
                normalize_path(&target, log_dir),
            ]
        } else {
            vec![
                kind.to_owned(),
                normalize_name(&name),
                "file".to_owned(),
                normalize_content(&fs::read_to_string(entry.path())?, log_dir),
            ]
        };
        rows.push(row);
    }
    rows.sort();
    Ok(rows)
}

#[test]
fn files_golden() -> TestResult {
    // glog's per-severity files under -log_dir, the cascade of lines into lower severities'
    // files, the file header, and the links beside the files and in -log_link.
    let log_dir = TempDir::new("glog-dir")?;
    let link_dir = TempDir::new("glog-link")?;
    let flags = Flags {
        log_dir: log_dir.path().display().to_string(),
        log_link: link_dir.path().display().to_string(),
        stderrthreshold: Severity::Fatal,
        ..Flags::default()
    };
    let (glog, stderr) = buffered_glog(test_env(log_dir.path()), flags);
    glog.logf(HERE, Severity::Info, "one", &[]);
    glog.logf(HERE, Severity::Warning, "two %d", &[Value::Int(2)]);
    glog.logf(HERE, Severity::Error, "three\nlines", &[]);
    glog.logf(HERE, Severity::Info, "four\n", &[]);
    glog.flush();
    check_eq(stderr.take().as_str(), "", "nothing reaches stderr")?;
    let dir_text = log_dir.path().display().to_string();
    let mut got = list_dir("dir", log_dir.path(), &dir_text)?;
    got.extend(list_dir("link", link_dir.path(), &dir_text)?);
    check_eq(&got, &read_golden("files.txt")?, "files")
}

static TICKS: AtomicI64 = AtomicI64::new(0);

/// A clock one second later at every reading.
fn ticking_clock() -> Time {
    Time::from_unix(1_232_673_803 + TICKS.fetch_add(1, Ordering::Relaxed), 0)
}

#[test]
fn rotation_links_the_files() -> TestResult {
    let dir = TempDir::new("glog-rotate")?;
    let env = Env {
        clock: ticking_clock,
        ..test_env(dir.path())
    };
    let flags = Flags {
        log_dir: dir.path().display().to_string(),
        stderrthreshold: Severity::Fatal,
        ..Flags::default()
    };
    let (glog, _stderr) = buffered_glog(env, flags);
    glog.set_max_size(300);
    for n in 0..8 {
        glog.logf(HERE, Severity::Info, "line %d", &[Value::Int(n)]);
    }
    glog.flush();
    let mut names: Vec<String> = fs::read_dir(dir.path())?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<_>>()?;
    names.sort();
    check(names.len() >= 3, format!("rotated files: {names:?}"))?;
    let mut continued = 0;
    let mut previous = 0;
    for name in names.iter().filter(|n| n.contains(".log.INFO.")) {
        let text = fs::read_to_string(dir.path().join(name))?;
        if text.ends_with("\nCONTINUED IN NEXT FILE\n") {
            continued += 1;
        }
        if !text.contains("Previous log: <none>\n") {
            previous += 1;
        }
    }
    check(continued >= 1, "a rotated file points to the next")?;
    check_eq(&previous, &continued, "each new file names the previous")
}

#[test]
fn error_lines_wake_the_flush_daemon() -> TestResult {
    let dir = TempDir::new("glog-daemon")?;
    let flags = Flags {
        log_dir: dir.path().display().to_string(),
        stderrthreshold: Severity::Fatal,
        ..Flags::default()
    };
    let (glog, _stderr) = buffered_glog(test_env(dir.path()), flags);
    glog.logf(HERE, Severity::Info, "buffered", &[]);
    let info = dir.path().join("golden.INFO");
    check(
        !fs::read_to_string(&info)?.contains("buffered"),
        "INFO stays buffered at -logbuflevel=0",
    )?;
    glog.logf(HERE, Severity::Error, "urgent", &[]);
    let error = dir.path().join("golden.ERROR");
    for _ in 0..200 {
        if fs::read_to_string(&error)?.contains("urgent") {
            return check(
                !fs::read_to_string(&info)?.contains("urgent"),
                "the daemon flushes ERROR and above only",
            );
        }
        thread::sleep(StdDuration::from_millis(10));
    }
    Err("the ERROR file was not flushed".into())
}

#[test]
fn bridge_maps_levels() -> TestResult {
    use log::Level;

    use super::GlogBridge;

    check_eq(
        &GlogBridge::severity(Level::Error),
        &Severity::Error,
        "error",
    )?;
    check_eq(
        &GlogBridge::severity(Level::Warn),
        &Severity::Warning,
        "warn",
    )?;
    check_eq(&GlogBridge::severity(Level::Info), &Severity::Info, "info")?;
    check_eq(
        &GlogBridge::severity(Level::Debug),
        &Severity::Info,
        "debug",
    )?;
    check_eq(
        &GlogBridge::severity(Level::Trace),
        &Severity::Info,
        "trace",
    )
}

/// The environment variable that turns a test into a child process's fatal scenario.
const CHILD: &str = "PBS_LOGGER_FATAL_CHILD";

/// Runs `test` of this binary as a child with [`CHILD`] set to `mode`.
fn run_child(test: &str, mode: &str) -> Result<(Option<i32>, String), Box<dyn Error>> {
    let out = Command::new(env::current_exe()?)
        .args(["--exact", test, "--test-threads=1"])
        .env(CHILD, mode)
        .output()?;
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt as _;
        out.status.signal()
    };
    #[cfg(not(unix))]
    let signal = None;
    Ok((signal, String::from_utf8_lossy(&out.stderr).into_owned()))
}

#[test]
fn fatal_child() {
    let Ok(mode) = env::var(CHILD) else {
        return;
    };
    let flags = Flags {
        logtostderr: true,
        ..Flags::default()
    };
    match mode.as_str() {
        "instance" => {
            let glog = Glog::new(test_env(&env::temp_dir()), flags, Box::new(io::stderr()));
            glog.fatalf(HERE, "boom %d", &[Value::Int(1)]);
        }
        "facade" => {
            super::global().set_flags(flags);
            crate::fatalf("fatal %s", &[Value::from("x")]);
        }
        "sinkfail" => {
            let missing = env::temp_dir().join("pbs-logger-missing-dir").join("x");
            let glog = Glog::new(
                Env {
                    temp_dir: missing.clone(),
                    ..test_env(&missing)
                },
                Flags {
                    log_dir: missing.display().to_string(),
                    ..Flags::default()
                },
                Box::new(io::stderr()),
            );
            glog.logf(HERE, Severity::Info, "lost", &[]);
        }
        _ => {}
    }
}

#[test]
fn fatal_logs_a_stack_then_aborts() -> TestResult {
    // Go: FatalDepthf writes the line with a stack dump, flushes, and sends SIGABRT.
    let (signal, stderr) = run_child("glog::tests::fatal_child", "instance")?;
    check(
        stderr.starts_with("F0123 01:23:23.123123    4242 here.rs:7] boom 1\n\n"),
        &stderr,
    )?;
    check(stderr.ends_with('\n'), "a final newline")?;
    if cfg!(unix) {
        check_eq(&signal, &Some(6), "SIGABRT")?;
    }
    let (_signal, facade_stderr) = run_child("glog::tests::fatal_child", "facade")?;
    check(facade_stderr.starts_with('F'), &facade_stderr)?;
    check(
        facade_stderr.contains(" logger.go:27] fatal x\n\n"),
        format!("the Go site of logger.Fatalf: {facade_stderr}"),
    )
}

#[test]
fn a_failed_write_is_fatal() -> TestResult {
    // Go: sinkf logs "log: exiting because of error writing previous log to sinks" and
    // aborts when a sink fails.
    let (signal, stderr) = run_child("glog::tests::fatal_child", "sinkfail")?;
    let missing = env::temp_dir().join("pbs-logger-missing-dir").join("x");
    let want = format!(
        "F0123 01:23:23.123123    4242 glog.go:261] log: exiting because of error writing \
         previous log to sinks: log: cannot create log: open {}/golden.HOST.USER.log.INFO.\
         20090123-012323.4242: no such file or directory\n\n",
        missing.display()
    );
    check(
        stderr.starts_with(&want),
        format!("{stderr}\nwant prefix {want}"),
    )?;
    if cfg!(unix) {
        check_eq(&signal, &Some(6), "SIGABRT")?;
    }
    Ok(())
}

#[test]
fn print_defaults_is_go_usage() -> TestResult {
    let usage = super::usage("prebid-server");
    let head = concat!(
        "Usage of prebid-server:\n",
        "  -alsologtostderr\n",
        "    \tlog to standard error as well as files\n",
    );
    check(usage.starts_with(head), &usage)?;
    check(
        usage.contains(
            "  -stderrthreshold value\n    \tlogs at or above this threshold go to stderr \
             (default 2)\n",
        ),
        &usage,
    )
}
