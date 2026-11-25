//! The facade against Prebid's logger package: its Go tests (`logger/logger_test.go` and
//! `logger/glog_test.go`) transliterated, `logger.Debugf`'s level (`logging.debug-level`,
//! restored) and `logger.Fatalf`'s exit (`logging.fatal`), the last two against
//! `testdata/golden/sinks.json`.
//!
//! Where Go swaps its mock logger in, a [`Capture`] keeps the lines; the mock keeps each call's
//! format and arguments apart, while a `tracing` event carries the formatted message, so the
//! transliterations check the message the arguments make. Where Go calls the real glog logger
//! and checks only that nothing panics, the lines go through the subscriber `-logtostderr` asks
//! for and are read back. Go's tests of the logger value itself (`TestDefaultLogger`,
//! `TestNewGlogLogger`'s depth, `TestGlogLogger_Depth` and the interface checks) have no
//! counterpart: the macros are the facade, with no logger value or interface behind them.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::process::{ExitCode, Termination as _};

    use pbs_logger::{
        Capture, FATAL_TARGET, Fatal, Severity, debugf, errorf, fatalf, infof, parse_command_line,
        subscriber, warnf,
    };
    use serde_json::Value;
    use tracing::subscriber::with_default;

    use super::support::{
        Buffer, PREBID, Scratch, TestResult, Written, check_eq, command_line, glog_lines, golden,
        list, log_levels, sinks_run, text, written,
    };

    /// The severities and messages `capture` kept.
    fn kept(capture: &Capture) -> Vec<(Severity, String)> {
        capture
            .lines()
            .into_iter()
            .map(|line| (line.severity, line.message))
            .collect()
    }

    /// `want` with owned messages.
    fn lines(want: &[(Severity, &str)]) -> Vec<(Severity, String)> {
        want.iter()
            .map(|&(severity, message)| (severity, message.to_owned()))
            .collect()
    }

    /// Logs `fatal` as `main` reports it, which is what `Fatalf` logs.
    fn report(fatal: Fatal) -> ExitCode {
        fatal.report()
    }

    /// Go: `TestDebug` (logger/logger_test.go:63-83). `debugf!` is an INFO line, as
    /// `GlogLogger.Debugf` calls `glog.InfoDepthf`.
    #[test]
    fn debug() -> TestResult {
        let capture = Capture::new();
        capture.run(|| {
            debugf!("debug message");
            debugf!("debug with args: {}, {}", "test", 123);
        });
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Info, "debug message"),
                (Severity::Info, "debug with args: test, 123"),
            ]),
            "two Debugf lines",
        )
    }

    /// Go: `TestInfo` (logger/logger_test.go:85-104).
    #[test]
    fn info() -> TestResult {
        let capture = Capture::new();
        capture.run(|| {
            infof!("info message");
            infof!("info with args: {}, {}", "test", 456);
        });
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Info, "info message"),
                (Severity::Info, "info with args: test, 456"),
            ]),
            "two Infof lines",
        )
    }

    /// Go: `TestWarn` (logger/logger_test.go:106-125).
    #[test]
    fn warn() -> TestResult {
        let capture = Capture::new();
        capture.run(|| {
            warnf!("warning message");
            warnf!("warning with args: {}, {}", "test", 789);
        });
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Warning, "warning message"),
                (Severity::Warning, "warning with args: test, 789"),
            ]),
            "two Warnf lines",
        )
    }

    /// Go: `TestError` (logger/logger_test.go:127-146).
    #[test]
    fn error() -> TestResult {
        let capture = Capture::new();
        capture.run(|| {
            errorf!("error message");
            errorf!("error with args: {}, {}", "test", 999);
        });
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Error, "error message"),
                (Severity::Error, "error with args: test, 999"),
            ]),
            "two Errorf lines",
        )
    }

    /// Go: `TestAllLogLevels` (logger/logger_test.go:148-170).
    #[test]
    fn all_log_levels() -> TestResult {
        let capture = Capture::new();
        let exit = capture.run(|| {
            debugf!("debug");
            infof!("info");
            warnf!("warn");
            errorf!("error");
            report(fatalf!("fatal"))
        });
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Info, "debug"),
                (Severity::Info, "info"),
                (Severity::Warning, "warn"),
                (Severity::Error, "error"),
                (Severity::Fatal, "fatal"),
            ]),
            "a line per level",
        )?;
        check_eq(&exit, &ExitCode::from(2), "Fatal's exit code")
    }

    /// Go: `TestEmptyMessages` (logger/logger_test.go:172-199).
    #[test]
    fn empty_messages() -> TestResult {
        let capture = Capture::new();
        let exit = capture.run(|| {
            debugf!("");
            infof!("");
            warnf!("");
            errorf!("");
            report(fatalf!(""))
        });
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Info, ""),
                (Severity::Info, ""),
                (Severity::Warning, ""),
                (Severity::Error, ""),
                (Severity::Fatal, ""),
            ]),
            "empty lines",
        )?;
        check_eq(&exit, &ExitCode::from(2), "Fatal's exit code")
    }

    /// Go: `TestMultipleArguments` (logger/logger_test.go:201-216), `%f` as `{:.6}`. Go's float
    /// is 3.14, which clippy takes for an approximation of π; 2.5 stands in for it.
    #[test]
    fn multiple_arguments() -> TestResult {
        let capture = Capture::new();
        capture.run(|| {
            infof!(
                "message: {}, number: {}, float: {:.6}, bool: {}",
                "test",
                42,
                2.5,
                true
            );
        });
        check_eq(
            &kept(&capture),
            &lines(&[(
                Severity::Info,
                "message: test, number: 42, float: 2.500000, bool: true",
            )]),
            "the arguments formatted",
        )
    }

    /// Go: `TestNoArgs` (logger/logger_test.go:218-245).
    #[test]
    fn no_args() -> TestResult {
        let capture = Capture::new();
        let exit = capture.run(|| {
            infof!("simple message");
            debugf!("simple debug");
            warnf!("simple warning");
            errorf!("simple error");
            report(fatalf!("simple fatal"))
        });
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Info, "simple message"),
                (Severity::Info, "simple debug"),
                (Severity::Warning, "simple warning"),
                (Severity::Error, "simple error"),
                (Severity::Fatal, "simple fatal"),
            ]),
            "the messages as given",
        )?;
        check_eq(&exit, &ExitCode::from(2), "Fatal's exit code")
    }

    /// Go: `TestSpecialCharacters` (logger/logger_test.go:264-278).
    #[test]
    fn special_characters() -> TestResult {
        let capture = Capture::new();
        capture.run(|| {
            infof!("message with special chars: \n\t\"quotes\" and 'apostrophes'");
        });
        check_eq(
            &kept(&capture),
            &lines(&[(
                Severity::Info,
                "message with special chars: \n\t\"quotes\" and 'apostrophes'",
            )]),
            "the characters kept",
        )
    }

    /// Go: `TestFatal` (logger/logger_test.go:285-304). The mock's `Fatalf` returns; here
    /// `fatalf!` builds the [`Fatal`] `main` returns, and its report logs the message.
    #[test]
    fn fatal() -> TestResult {
        let plain = fatalf!("fatal message");
        let with_args = fatalf!("fatal with args: {}, {}", "test", 111);
        check_eq(
            &(plain.message(), with_args.to_string()),
            &("fatal message", "fatal with args: test, 111".to_owned()),
            "the messages",
        )?;
        let capture = Capture::new();
        let exits = capture.run(|| [report(plain), report(with_args)]);
        check_eq(
            &kept(&capture),
            &lines(&[
                (Severity::Fatal, "fatal message"),
                (Severity::Fatal, "fatal with args: test, 111"),
            ]),
            "two Fatalf lines",
        )?;
        check_eq(&exits, &[ExitCode::from(2); 2], "Fatal's exit codes")
    }

    /// Go: `TestWithRealGlogLogger` (logger/logger_test.go:247-262) and `TestGlogLogger_Debug`
    /// to `TestGlogLogger_SpecialCharacters` (logger/glog_test.go:28-171): the
    /// calls under `-logtostderr` and `-v 2`, which Go checks do not panic; here every line is
    /// read back from stderr.
    #[test]
    fn real_logger() -> TestResult {
        let mut usage = Buffer::default();
        let flags = parse_command_line(&command_line(&["-logtostderr", "-v", "2"]), &mut usage)
            .map_err(|exit| format!("{exit:?}: {}", usage.text()))?
            .flags;
        let stderr = Buffer::default();
        let scratch = Scratch::new()?;
        let (built, logging) = subscriber(&flags, "prebid-server", scratch.path(), stderr.clone())?;
        with_default(built, || {
            debugf!(target: PREBID, "debug message with args: {}, {}", "test", 123);
            infof!(target: PREBID, "info message with args: {}, {}", "test", 456);
            warnf!(target: PREBID, "warning message with args: {}, {}", "test", 789);
            errorf!(target: PREBID, "error message with args: {}, {}", "test", 999);
            infof!(target: PREBID, "");
            infof!(
                target: PREBID,
                "message: {}, number: {}, float: {:.6}, bool: {}",
                "test",
                42,
                2.5,
                true
            );
        });
        drop(logging);
        let read: Vec<(String, String)> = written(&stderr.text())?
            .iter()
            .map(Written::glog_like)
            .collect();
        check_eq(
            read.as_slice(),
            &[
                ("I", "debug message with args: test, 123"),
                ("I", "info message with args: test, 456"),
                ("W", "warning message with args: test, 789"),
                ("E", "error message with args: test, 999"),
                ("I", ""),
                (
                    "I",
                    "message: test, number: 42, float: 2.500000, bool: true",
                ),
            ]
            .map(|(letter, message)| (letter.to_owned(), message.to_owned()))[..],
            "every line on stderr",
        )
    }

    /// `logger.Debugf` writes glog INFO lines (`I`), which the image's stderr shows. tracing
    /// has a DEBUG level below INFO, which the subscriber drops for Prebid's crates and which
    /// would write `DEBUG`; `debugf!` logs at INFO instead, so its lines are kept and written as
    /// INFO lines, as `Infof`'s are.
    // Go: logger/glog.go:13-15 (GlogLogger.Debugf calls glog.InfoDepthf)
    #[test]
    fn debugf_is_info() -> TestResult {
        let sinks = golden("sinks.json")?;
        let run = sinks_run(&sinks, "levels", &["-logtostderr"])?;
        let go = glog_lines(run, "/stderr")?;
        check_eq(
            &go.first(),
            &Some(&("I".to_owned(), "debugf 1".to_owned())),
            "Go: Debugf's line is an INFO line",
        )?;

        let capture = Capture::new();
        capture.run(|| debugf!(target: PREBID, "debugf {}", 1));
        check_eq(
            &kept(&capture),
            &lines(&[(Severity::Info, "debugf 1")]),
            "Rust: debugf! is an INFO event",
        )?;

        let mut usage = Buffer::default();
        let flags = parse_command_line(&command_line(&["-logtostderr"]), &mut usage)
            .map_err(|exit| format!("{exit:?}: {}", usage.text()))?
            .flags;
        let stderr = Buffer::default();
        let scratch = Scratch::new()?;
        let (built, logging) = subscriber(&flags, "prebid-server", scratch.path(), stderr.clone())?;
        with_default(built, log_levels);
        drop(logging);
        let rust: Vec<(String, String)> = written(&stderr.text())?
            .iter()
            .map(|line| (line.level.clone(), line.message.clone()))
            .collect();
        check_eq(
            &rust.first(),
            &Some(&("INFO".to_owned(), "debugf 1".to_owned())),
            "Rust: the line is written as INFO",
        )
    }

    /// `logger.Fatalf` writes glog's FATAL line, then every goroutine's stack and the runtime's
    /// `SIGABRT` line, and the process exits 2 where it stands. Here `fatalf!` builds a
    /// [`Fatal`] that `main` returns: its report writes one ERROR line at [`FATAL_TARGET`], which
    /// reaches stderr at every `-stderrthreshold` as glog's FATAL line does, with no stack, and
    /// `main` exits 2 once the error has come up to it.
    // Go: logger/glog.go:33-35 (glog.FatalDepthf); glog v1.2.5, glog.go:656-671 (ctxfatalf and
    // flushAndAbort) and glog_file_linux.go:33-39 (the SIGABRT the Go runtime answers with the
    // stacks and exit code 2)
    #[test]
    fn fatal_exit_code() -> TestResult {
        let sinks = golden("sinks.json")?;
        for args in [&["-logtostderr"][..], &[], &["-stderrthreshold=FATAL"]] {
            let run = sinks_run(&sinks, "fatal", args)?;
            let kinds: Vec<String> = list(run, "/stderr")?
                .iter()
                .map(|entry| text(entry, "/kind").map(str::to_owned))
                .collect::<TestResult<_>>()?;
            check_eq(
                kinds.as_slice(),
                &["glog", "stack", "signal", "stack"].map(str::to_owned)[..],
                &format!("Go {args:?}: the FATAL line, stacks and SIGABRT"),
            )?;
            check_eq(
                &run.pointer("/exit").and_then(Value::as_u64),
                &Some(2),
                &format!("Go {args:?}: exit code 2"),
            )?;
        }

        let capture = Capture::new();
        let exit = capture.run(|| report(fatalf!("fatalf {}", 5)));
        check_eq(&exit, &ExitCode::from(2), "Rust: exit code 2")?;
        let kept_lines = capture.lines();
        check_eq(
            &kept_lines
                .iter()
                .map(|line| (line.severity, line.target.as_str(), line.message.as_str()))
                .collect::<Vec<_>>(),
            &vec![(Severity::Fatal, FATAL_TARGET, "fatalf 5")],
            "Rust: one FATAL line at the fatal target",
        )
    }
}
