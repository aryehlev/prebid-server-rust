//! Ports of `logger/logger_test.go` and `logger/glog_test.go`, and the lines the package
//! functions, the default logger and the `log` bridge write through the process-wide glog.
//!
//! The package logger and the process-wide glog are shared state, so every test that touches
//! them runs inside [`process_wide_logging`], one after another.

use std::{
    env,
    panic::Location,
    process,
    sync::{Arc, Mutex, PoisonError},
};

use pbs_gocompat::fmt::Value;

use crate::{
    GlogLogger, Logger, debugf, errorf, fatalf,
    glog::{self, Env, Flags, Glog},
    infof, reset_logger, set_logger,
    testutil::{SharedBuf, TestResult, check, check_eq, fixed_time},
    uses_default_logger, warnf,
};

/// A package function such as [`infof()`].
type PackageFn = fn(&str, &[Value]);

/// A glog function over operands such as [`glog::infoln`].
type OperandFn = fn(&[Value]);

/// Go: `logCall`.
#[derive(Clone, Debug, PartialEq)]
struct LogCall {
    msg: String,
    args: Vec<Value>,
}

/// Go: `mockLogger`: records each call by level.
#[derive(Debug, Default)]
struct MockLogger {
    calls: Mutex<Vec<(&'static str, LogCall)>>,
}

impl MockLogger {
    fn record(&self, level: &'static str, msg: &str, args: &[Value]) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                level,
                LogCall {
                    msg: msg.to_owned(),
                    args: args.to_vec(),
                },
            ));
    }

    fn calls(&self, level: &str) -> Vec<LogCall> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(l, _)| *l == level)
            .map(|(_, c)| c.clone())
            .collect()
    }
}

impl Logger for MockLogger {
    fn debugf(&self, msg: &str, args: &[Value]) {
        self.record("debug", msg, args);
    }

    fn infof(&self, msg: &str, args: &[Value]) {
        self.record("info", msg, args);
    }

    fn warnf(&self, msg: &str, args: &[Value]) {
        self.record("warn", msg, args);
    }

    fn errorf(&self, msg: &str, args: &[Value]) {
        self.record("error", msg, args);
    }

    fn fatalf(&self, msg: &str, args: &[Value]) {
        self.record("fatal", msg, args);
    }
}

fn call(msg: &str, args: &[Value]) -> LogCall {
    LogCall {
        msg: msg.to_owned(),
        args: args.to_vec(),
    }
}

/// Go: `TestDebug`, `TestInfo`, `TestWarn`, `TestError` and `TestFatal`: each package function
/// passes the message and arguments to the installed logger unformatted.
fn package_functions_reach_the_installed_logger() -> TestResult {
    let mock = Arc::new(MockLogger::default());
    let installed: Arc<dyn Logger> = Arc::<MockLogger>::clone(&mock);
    set_logger(installed);
    check(!uses_default_logger(), "the mock is installed")?;
    let cases: [(&str, PackageFn, &str, Value, i64); 5] = [
        ("debug", debugf, "debug", Value::from("test"), 123),
        ("info", infof, "info", Value::from("test"), 456),
        ("warn", warnf, "warn", Value::from("test"), 789),
        ("error", errorf, "error", Value::from("test"), 999),
        ("fatal", fatalf, "fatal", Value::from("test"), 111),
    ];
    for (level, package_fn, word, text, n) in cases {
        let plain = format!("{word} message");
        let with_args = format!("{word} with args: %s, %d");
        package_fn(&plain, &[]);
        package_fn(&with_args, &[text.clone(), Value::Int(n)]);
        check_eq(
            &mock.calls(level),
            &vec![call(&plain, &[]), call(&with_args, &[text, Value::Int(n)])],
            level,
        )?;
    }

    // Go: TestEmptyMessages, TestNoArgs, TestMultipleArguments and TestSpecialCharacters.
    debugf("", &[]);
    let many = [
        Value::from("a"),
        Value::Int(1),
        Value::Float64(2.5),
        Value::Bool(true),
        Value::Nil,
    ];
    infof("%s %d %f %t %v", &many);
    let special = "special: \n\t\"quotes\" and 'apostrophes' and %%";
    warnf(special, &[]);
    check_eq(&mock.calls("debug").pop(), &Some(call("", &[])), "empty")?;
    check_eq(
        &mock.calls("info").pop(),
        &Some(call("%s %d %f %t %v", &many)),
        "many arguments",
    )?;
    check_eq(
        &mock.calls("warn").pop(),
        &Some(call(special, &[])),
        "special",
    )?;

    // The macros pass the message and their arguments, converted by Value::from, unchanged.
    crate::debugf!("macro %s %d", "test", 1_i64);
    crate::infof!("macro %s %d", String::from("test"), 2_isize,);
    crate::warnf!("macro");
    crate::errorf!("macro %v %v", Value::Nil, 2.5_f64);
    crate::fatalf!("macro %t", true);
    let macro_cases = [
        (
            "debug",
            call("macro %s %d", &[Value::from("test"), Value::Int64(1)]),
        ),
        (
            "info",
            call("macro %s %d", &[Value::from("test"), Value::Int(2)]),
        ),
        ("warn", call("macro", &[])),
        (
            "error",
            call("macro %v %v", &[Value::Nil, Value::Float64(2.5)]),
        ),
        ("fatal", call("macro %t", &[Value::Bool(true)])),
    ];
    for (level, want) in macro_cases {
        check_eq(&mock.calls(level).pop(), &Some(want), level)?;
    }
    reset_logger();
    check(
        uses_default_logger(),
        "Go: TestDefaultLogger after restoring",
    )
}

/// A [`Logger`] method.
#[derive(Clone, Copy, Debug)]
enum Method {
    Debug,
    Info,
    Warn,
    Error,
}

/// Calls a [`Logger`] method and returns the line of the call, which the logger reports.
#[track_caller]
fn log_here(logger: &dyn Logger, method: Method, msg: &str, args: &[Value]) -> u32 {
    match method {
        Method::Debug => logger.debugf(msg, args),
        Method::Info => logger.infof(msg, args),
        Method::Warn => logger.warnf(msg, args),
        Method::Error => logger.errorf(msg, args),
    }
    Location::caller().line()
}

/// The default logger writes through glog at the Go call sites of `logger/logger.go`.
fn default_logger_reports_go_sites(stderr: &SharedBuf) -> TestResult {
    reset_logger();
    let cases: [(PackageFn, char, &str); 4] = [
        (debugf, 'I', "logger.go:7] "),
        (infof, 'I', "logger.go:12] "),
        (warnf, 'W', "logger.go:17] "),
        (errorf, 'E', "logger.go:22] "),
    ];
    let pid = process::id();
    for (package_fn, letter, site) in cases {
        package_fn(
            "bidder %s: %d bids",
            &[Value::from("appnexus"), Value::Int(2)],
        );
        let line = stderr.take();
        check(line.starts_with(letter), &line)?;
        check(
            line.ends_with(&format!(" {pid:>7} {site}bidder appnexus: 2 bids\n")),
            format!("{line:?} at {site}"),
        )?;
    }
    Ok(())
}

/// Go: `TestWithRealGlogLogger` and the `GlogLogger` tests on the process-wide glog: direct
/// calls report the Rust caller, whatever the depth.
fn glog_logger_reports_its_caller(stderr: &SharedBuf) -> TestResult {
    let args = [Value::from("test"), Value::Int(456)];
    let at = log_here(
        &GlogLogger::new(),
        Method::Info,
        "info message with args: %s, %d",
        &args,
    );
    let info = stderr.take();
    check(
        info.ends_with(&format!(
            " tests.rs:{at}] info message with args: test, 456\n"
        )),
        &info,
    )?;
    let deep_at = log_here(
        &GlogLogger::with_depth(2),
        Method::Warn,
        "info with custom depth",
        &[],
    );
    let deep = stderr.take();
    check(
        deep.starts_with('W')
            && deep.ends_with(&format!(" tests.rs:{deep_at}] info with custom depth\n")),
        &deep,
    )
}

/// The `log` crate's records become glog lines at the record's site, and glog's own
/// functions report their caller.
fn log_bridge_writes_glog_lines(stderr: &SharedBuf) -> TestResult {
    glog::install_log_bridge(log::LevelFilter::Debug).map_err(|e| e.to_string())?;
    {
        let at = line!() + 1;
        log::error!("Error updating conversion rates: {}", "timeout");
        let out = stderr.take();
        check(
            out.starts_with('E')
                && out.ends_with(&format!(
                    " tests.rs:{at}] Error updating conversion rates: timeout\n"
                )),
            &out,
        )?;
    }
    log::debug!("debug %d is not a format");
    let debug = stderr.take();
    check(
        debug.starts_with('I') && debug.ends_with("] debug %d is not a format\n"),
        &debug,
    )?;
    log::trace!("dropped");
    check_eq(
        stderr.take().as_str(),
        "",
        "trace is above the bridge's level",
    )?;
    {
        let at = line!() + 1;
        glog::warningf("glog %s", &[Value::from("direct")]);
        let out = stderr.take();
        check(
            out.starts_with('W') && out.ends_with(&format!(" tests.rs:{at}] glog direct\n")),
            &out,
        )?;
    }
    let cases: [(OperandFn, Vec<Value>, &str); 6] = [
        (
            glog::infoln,
            vec![Value::from("rule"), Value::Int(3)],
            "I] rule 3\n",
        ),
        (glog::errorln, vec![], "E] \n"),
        (
            glog::info,
            vec![
                Value::from("a"),
                Value::Int(1),
                Value::Int(2),
                Value::from("b"),
            ],
            "I] a1 2b\n",
        ),
        (
            glog::warning,
            vec![Value::Int(3), Value::Int(4)],
            "W] 3 4\n",
        ),
        (glog::error, vec![Value::from("x")], "E] x\n"),
        (glog::warningln, vec![Value::from("w")], "W] w\n"),
    ];
    for (log_fn, args, want) in cases {
        log_fn(&args);
        let out = stderr.take();
        let letter = want.get(..1).unwrap_or_default();
        let tail = want.get(1..).unwrap_or_default();
        check(
            out.starts_with(letter) && out.ends_with(tail),
            format!("{out:?}, want {want:?}"),
        )?;
    }
    glog::infof("%s", &[Value::from("i")]);
    glog::errorf("%s", &[Value::from("e")]);
    let both = stderr.take();
    check(
        both.starts_with('I') && both.contains("] i\nE") && both.ends_with("] e\n"),
        &both,
    )?;
    glog::flush();
    Ok(())
}

#[test]
fn process_wide_logging() -> TestResult {
    let stderr = SharedBuf::default();
    glog::global().set_stderr(Box::new(stderr.clone()));
    glog::global().set_flag("logtostderr", "true")?;
    package_functions_reach_the_installed_logger()?;
    default_logger_reports_go_sites(&stderr)?;
    glog_logger_reports_its_caller(&stderr)?;
    log_bridge_writes_glog_lines(&stderr)
}

/// A glog writing into a buffer at the fixed time, for the `GlogLogger` level tests.
fn buffered_logger() -> (GlogLogger, SharedBuf) {
    let stderr = SharedBuf::default();
    let env = Env {
        program: "test".to_owned(),
        host: "host".to_owned(),
        user: "user".to_owned(),
        pid: 7,
        temp_dir: env::temp_dir(),
        clock: fixed_time,
    };
    let flags = Flags {
        logtostderr: true,
        ..Flags::default()
    };
    let glog = Glog::new(env, flags, Box::new(stderr.clone()));
    (GlogLogger::with_glog(glog), stderr)
}

#[test]
fn new_glog_logger_has_depth_one() -> TestResult {
    // Go: TestNewGlogLogger and TestGlogLogger_ImplementsLoggerInterface.
    let logger = GlogLogger::new();
    check_eq(&logger.depth(), &1, "Default depth should be 1")?;
    check_eq(&GlogLogger::default().depth(), &1, "Default")?;
    check_eq(&GlogLogger::with_depth(2).depth(), &2, "custom depth")?;
    let _as_logger: &dyn Logger = &logger;
    Ok(())
}

#[test]
fn glog_logger_levels() -> TestResult {
    // Go: TestGlogLogger_Debug, _Info, _Warn, _Error, _AllLevels, _EmptyMessage, _NoArgs,
    // _MultipleArgs and _SpecialCharacters, with the lines they write checked: Debugf logs
    // at INFO.
    let (logger, stderr) = buffered_logger();
    let special = "message with special chars: \n\t\"quotes\" and 'apostrophes'";
    let multiple = [
        Value::from("test"),
        Value::Int(42),
        Value::Float64(2.75),
        Value::Bool(true),
    ];
    let cases: [(Method, &str, &[Value], &str); 6] = [
        (
            Method::Debug,
            "debug message with args: %s, %d",
            &[Value::from("test"), Value::Int(123)],
            "I debug message with args: test, 123",
        ),
        (Method::Info, "", &[], "I "),
        (Method::Warn, "simple warning", &[], "W simple warning"),
        (Method::Error, "simple error", &[], "E simple error"),
        (
            Method::Info,
            "message: %s, number: %d, float: %f, bool: %v",
            &multiple,
            "I message: test, number: 42, float: 2.750000, bool: true",
        ),
        (Method::Info, special, &[], ""),
    ];
    for (method, msg, args, want) in cases {
        let at = log_here(&logger, method, msg, args);
        let (letter, text) = want.split_once(' ').unwrap_or(("I", special));
        check_eq(
            stderr.take().as_str(),
            format!("{letter}0123 01:23:23.123123       7 tests.rs:{at}] {text}\n").as_str(),
            msg,
        )?;
    }
    Ok(())
}
