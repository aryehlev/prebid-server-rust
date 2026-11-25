//! The lines against glog's (`testdata/golden/sinks.json`, which `testdata/go/sinks` writes):
//! tracing-subscriber's layout where glog writes its header (`logging.line-format`), what each
//! does to a long message and to one that ends with a newline (`logging.message-bytes`) and to
//! control characters (`logging.control-characters`), and the lines of crates other than Prebid
//! Server's own where Go's standard logger writes its own (`logging.std-log-lines`).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{fmt::Write as _, fs};

    use pbs_logger::{GlogFlags, infof, parse_command_line, subscriber, warnf};
    use serde_json::Value;
    use tracing::{debug, info, subscriber::with_default, warn};

    use super::support::{
        Buffer, PREBID, Scratch, TestResult, Written, check, check_eq, command_line, glog_lines,
        golden, list, log_levels, sinks_run, text, written,
    };

    /// The flags of `args`.
    fn flags(args: &[&str]) -> TestResult<GlogFlags> {
        let mut stderr = Buffer::default();
        parse_command_line(&command_line(args), &mut stderr)
            .map(|command_line| command_line.flags)
            .map_err(|exit| format!("{args:?}: {exit:?}: {}", stderr.text()).into())
    }

    /// `line` with tracing-subscriber's RFC 3339 time, checked digit by digit, replaced by
    /// `YYYY-MM-DDThh:mm:ss.uuuuuuZ`.
    fn without_time(line: &str) -> TestResult<String> {
        const SHAPE: &str = "dddd-dd-ddTdd:dd:dd.ddddddZ";
        let time = line
            .get(..SHAPE.len())
            .ok_or("the line is shorter than its time")?;
        let shaped = time.bytes().zip(SHAPE.bytes()).all(|(byte, shape)| {
            if shape == b'd' {
                byte.is_ascii_digit()
            } else {
                byte == shape
            }
        });
        check(shaped, &format!("{line:?} starts with an RFC 3339 time"))?;
        Ok(line.replacen(time, "YYYY-MM-DDThh:mm:ss.uuuuuuZ", 1))
    }

    /// glog's line for a sinks golden entry, in the layout its files' header gives
    /// (`Log line format: [IWEF]mmdd hh:mm:ss.uuuuuu threadid file:line] msg`).
    fn glog_line(entry: &Value) -> TestResult<String> {
        Ok(format!(
            "{}mmdd hh:mm:ss.uuuuuu threadid {}] {}",
            text(entry, "/severity")?,
            text(entry, "/file_line")?,
            text(entry, "/message")?
        ))
    }

    /// glog writes `Lmmdd hh:mm:ss.uuuuuu threadid file:line] msg`, where through the facade
    /// (depth 1) the file and line are the facade's own: `logger.go:7` for every `Debugf`,
    /// `:12`, `:17` and `:22` for the others. tracing-subscriber's fmt layer writes an RFC 3339
    /// time to the microsecond, the level right-aligned in five characters, the target (the
    /// calling module's path) and the message.
    // Go: logger/logger.go:6-23 and logger/glog.go:13-30 with NewGlogLogger's depth 1
    // (logger/glog.go:37-41); glog v1.2.5, internal/logsink/logsink.go:204-245 (the line's
    // header) and glog_file.go:332-338 (the file's)
    #[test]
    fn line_layout() -> TestResult {
        let sinks = golden("sinks.json")?;
        let run = sinks_run(&sinks, "levels", &["-logtostderr"])?;
        let go = list(run, "/stderr")?
            .iter()
            .map(glog_line)
            .collect::<TestResult<Vec<String>>>()?;
        check_eq(
            go.as_slice(),
            &[
                "Immdd hh:mm:ss.uuuuuu threadid logger.go:7] debugf 1",
                "Immdd hh:mm:ss.uuuuuu threadid logger.go:12] infof 2",
                "Wmmdd hh:mm:ss.uuuuuu threadid logger.go:17] warnf 3",
                "Emmdd hh:mm:ss.uuuuuu threadid logger.go:22] errorf 4",
            ]
            .map(str::to_owned)[..],
            "glog's lines through the facade",
        )?;
        let with_file = sinks_run(&sinks, "levels", &["-log_dir=LOGDIR"])?;
        let header = list(with_file, "/files")?
            .iter()
            .find_map(|file| file.pointer("/header/4").and_then(Value::as_str));
        check_eq(
            &header,
            &Some("Log line format: [IWEF]mmdd hh:mm:ss.uuuuuu threadid file:line] msg"),
            "the layout glog's files give",
        )?;

        let stderr = Buffer::default();
        let scratch = Scratch::new()?;
        let (built, logging) = subscriber(
            &flags(&["-logtostderr"])?,
            "prebid-server",
            scratch.path(),
            stderr.clone(),
        )?;
        with_default(built, log_levels);
        drop(logging);
        let rust = stderr
            .text()
            .lines()
            .map(without_time)
            .collect::<TestResult<Vec<String>>>()?;
        check_eq(
            rust.as_slice(),
            &[
                "YYYY-MM-DDThh:mm:ss.uuuuuuZ  INFO pbs_server: debugf 1",
                "YYYY-MM-DDThh:mm:ss.uuuuuuZ  INFO pbs_server: infof 2",
                "YYYY-MM-DDThh:mm:ss.uuuuuuZ  WARN pbs_server: warnf 3",
                "YYYY-MM-DDThh:mm:ss.uuuuuuZ ERROR pbs_server: errorf 4",
            ]
            .map(str::to_owned)[..],
            "tracing-subscriber's lines",
        )
    }

    /// `text` cut after each newline as the sinks golden's `raw` is: each piece with its time
    /// and thread id as placeholders, every run of more than 16 x's written `x*N`, and its length
    /// in bytes.
    fn pieces(text: &str, time: fn(&str) -> TestResult<String>) -> TestResult<Vec<(String, u64)>> {
        text.split_inclusive('\n')
            .map(|piece| {
                let normal = if piece == "\n" {
                    piece.to_owned()
                } else {
                    time(piece)?
                };
                Ok((count_runs(&normal), u64::try_from(piece.len())?))
            })
            .collect()
    }

    /// `text` with every run of more than 16 x's written `x*N`.
    fn count_runs(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find('x') {
            let (before, from) = rest.split_at(at);
            out.push_str(before);
            let run = from.len() - from.trim_start_matches('x').len();
            let (xs, after) = from.split_at(run);
            if run > 16 {
                write!(out, "x*{run}").unwrap_or_default();
            } else {
                out.push_str(xs);
            }
            rest = after;
        }
        out.push_str(rest);
        out
    }

    /// glog cuts a line at `MaxLogMessageLen`, 15,000 bytes with its header and newline, and
    /// adds no newline to a message that ends with one, which some of Prebid's calls do
    /// (router/router.go:133 and :140, config/config.go:767). tracing-subscriber writes the whole
    /// message and always ends the line with a newline of its own, so such a message leaves an
    /// empty line after it.
    // Go: glog v1.2.5, internal/logsink/logsink.go:29-33 and 247-254
    #[test]
    fn message_bytes() -> TestResult {
        let sinks = golden("sinks.json")?;
        let long = sinks
            .pointer("/long_message_length")
            .and_then(Value::as_u64)
            .ok_or("no long message length")?;
        let newline = text(&sinks, "/newline_message")?;
        let run = sinks_run(&sinks, "message", &["-logtostderr"])?;
        let go = list(run, "/raw")?
            .iter()
            .map(|piece| {
                let bytes = piece.pointer("/bytes").and_then(Value::as_u64);
                Ok((text(piece, "/text")?.to_owned(), bytes.ok_or("no bytes")?))
            })
            .collect::<TestResult<Vec<(String, u64)>>>()?;
        check_eq(
            go.as_slice(),
            &[
                (
                    "Immdd hh:mm:ss.uuuuuu threadid logger.go:12] x*14955\n".to_owned(),
                    15_000,
                ),
                (
                    "Immdd hh:mm:ss.uuuuuu threadid logger.go:12] ends with a newline\n".to_owned(),
                    64,
                ),
            ][..],
            "Go: the long line cut to 15,000 bytes, one newline after the other",
        )?;

        let stderr = Buffer::default();
        let scratch = Scratch::new()?;
        let (built, logging) = subscriber(
            &flags(&["-logtostderr"])?,
            "prebid-server",
            scratch.path(),
            stderr.clone(),
        )?;
        let message = "x".repeat(usize::try_from(long)?);
        with_default(built, || {
            infof!(target: PREBID, "{message}");
            infof!(target: PREBID, "{newline}");
        });
        drop(logging);
        check_eq(
            pieces(&stderr.text(), without_time)?.as_slice(),
            &[
                (
                    "YYYY-MM-DDThh:mm:ss.uuuuuuZ  INFO pbs_server: x*20000\n".to_owned(),
                    20_047,
                ),
                (
                    "YYYY-MM-DDThh:mm:ss.uuuuuuZ  INFO pbs_server: ends with a newline\n"
                        .to_owned(),
                    66,
                ),
                ("\n".to_owned(), 1),
            ][..],
            "Rust: the whole message, and an empty line after the newline",
        )
    }

    /// glog writes a message's control characters as `fmt.Sprintf` made them; tracing-subscriber's
    /// fmt layer writes ESC, BEL, BS, FF, DEL and the C1 controls (U+0080 to U+009F) as escapes, so
    /// that a message cannot drive a terminal, and the other control characters as they are.
    // Go: glog v1.2.5, internal/logsink/logsink.go:247-258 (the message formatted into the line
    // and handed to the sinks as it is); tracing-subscriber 0.3.23, src/fmt/format/mod.rs:445 and
    // 1316-1323 and src/fmt/format/escape.rs:22-44 (the sanitizing it does by default)
    #[test]
    fn control_characters() -> TestResult {
        const ESCAPED: &str = "esc \\x1b[31m bel \\x07 bs \\x08 ff \\x0c del \\x7f nel \\u{85} csi \
                               \\u{9b} tab \t vt \x0b cr \r nul \0 end";
        let sinks = golden("sinks.json")?;
        let message = text(&sinks, "/control_message")?;
        check_eq(
            message,
            "esc \x1b[31m bel \x07 bs \x08 ff \x0c del \x7f nel \u{85} csi \u{9b} tab \t vt \
             \x0b cr \r nul \0 end",
            "the golden's message",
        )?;
        let run = sinks_run(&sinks, "control", &["-logtostderr"])?;
        let go = list(run, "/raw")?
            .iter()
            .map(|piece| text(piece, "/text").map(str::to_owned))
            .collect::<TestResult<Vec<String>>>()?;
        check_eq(
            go.as_slice(),
            &[format!(
                "Immdd hh:mm:ss.uuuuuu threadid logger.go:12] {message}\n"
            )][..],
            "Go: the message as it is",
        )?;

        let stderr = Buffer::default();
        let scratch = Scratch::new()?;
        let (built, logging) = subscriber(
            &flags(&["-logtostderr"])?,
            "prebid-server",
            scratch.path(),
            stderr.clone(),
        )?;
        with_default(built, || infof!(target: PREBID, "{message}"));
        drop(logging);
        let rust: Vec<String> = pieces(&stderr.text(), without_time)?
            .into_iter()
            .map(|(piece, _)| piece)
            .collect();
        check_eq(
            rust.as_slice(),
            &[format!(
                "YYYY-MM-DDThh:mm:ss.uuuuuuZ  INFO {PREBID}: {ESCAPED}\n"
            )][..],
            "Rust: ESC, BEL, BS, FF, DEL and the C1 controls escaped, the rest as they are",
        )
    }

    /// What one Rust run wrote: stderr's lines and the log file's.
    struct Wrote {
        stderr: Vec<Written>,
        file: Vec<Written>,
    }

    /// Logs as a dependency and as Prebid Server under `args`: the standard logger's message at
    /// WARN, an INFO and a DEBUG line from a crate other than Prebid's, then `logger.Warnf`'s.
    fn dependency_run(args: &[&str]) -> TestResult<Wrote> {
        let sinks = golden("sinks.json")?;
        let message = text(&sinks, "/std_message")?.to_owned();
        let stderr = Buffer::default();
        let scratch = Scratch::new()?;
        let (built, logging) = subscriber(
            &flags(args)?,
            "prebid-server",
            scratch.path(),
            stderr.clone(),
        )?;
        with_default(built, || {
            warn!(target: "h2::proto::connection", "{message}");
            info!(target: "h2::proto::connection", "an INFO line of a dependency");
            debug!(target: "h2::proto::connection", "a DEBUG line of a dependency");
            warnf!(target: PREBID, "warnf {}", 3);
        });
        drop(logging);
        let mut file = Vec::new();
        for name in Scratch::names(scratch.path())? {
            let text = fs::read_to_string(scratch.path().join(name))?;
            file.extend(written(&text)?);
        }
        Ok(Wrote {
            stderr: written(&stderr.text())?,
            file,
        })
    }

    /// The level, target and message of each line.
    fn shown(lines: &[Written]) -> Vec<(String, String, String)> {
        lines
            .iter()
            .map(|line| {
                (
                    line.level.clone(),
                    line.target.clone(),
                    line.message.clone(),
                )
            })
            .collect()
    }

    /// Go's standard logger, which net/http's servers write through (`http: TLS handshake
    /// error`), writes its own `yyyy/mm/dd hh:mm:ss message` lines to stderr whatever glog's
    /// flags say and never to glog's files. Here crates other than Prebid Server's own log
    /// through the same subscriber as Prebid's code: from WARN up, as tracing-subscriber lines,
    /// to stderr at `-stderrthreshold` and to the log file like any other line.
    // Go: log/log.go:50 and 87 (the standard logger: Ldate|Ltime to os.Stderr) and
    // net/http/server.go:1943 and 3646-3652 (the TLS handshake error, through the server's
    // ErrorLog or the standard logger)
    #[test]
    fn dependency_lines() -> TestResult {
        let sinks = golden("sinks.json")?;
        let message = text(&sinks, "/std_message")?;
        let plain = sinks_run(&sinks, "stdlog", &[])?;
        let kinds = |run: &Value| -> TestResult<Vec<(String, String)>> {
            list(run, "/stderr")?
                .iter()
                .map(|entry| {
                    Ok((
                        text(entry, "/kind")?.to_owned(),
                        text(entry, "/message")?.to_owned(),
                    ))
                })
                .collect()
        };
        check_eq(
            &kinds(plain)?,
            &vec![("std".to_owned(), message.to_owned())],
            "Go: the standard logger's line reaches stderr under the default threshold",
        )?;
        for file in list(plain, "/files")? {
            if file.pointer("/lines").is_some() {
                check_eq(
                    &glog_lines(file, "/lines")?,
                    &vec![("W".to_owned(), "warnf 3".to_owned())],
                    "Go: glog's files hold only glog's line",
                )?;
            }
        }
        let to_stderr = sinks_run(&sinks, "stdlog", &["-logtostderr"])?;
        check_eq(
            &kinds(to_stderr)?,
            &vec![
                ("std".to_owned(), message.to_owned()),
                ("glog".to_owned(), "warnf 3".to_owned()),
            ],
            "Go: under -logtostderr",
        )?;

        let kept = vec![
            (
                "WARN".to_owned(),
                "h2::proto::connection".to_owned(),
                message.to_owned(),
            ),
            ("WARN".to_owned(), PREBID.to_owned(), "warnf 3".to_owned()),
        ];
        let default = dependency_run(&[])?;
        check_eq(
            &shown(&default.stderr),
            &Vec::new(),
            "Rust: no WARN line reaches stderr under the default threshold",
        )?;
        check_eq(
            &shown(&default.file),
            &kept,
            "Rust: the file holds both WARN lines and neither the INFO nor the DEBUG one",
        )?;
        let logtostderr = dependency_run(&["-logtostderr"])?;
        check_eq(
            &shown(&logtostderr.stderr),
            &kept,
            "Rust: under -logtostderr",
        )?;
        check(
            logtostderr.file.is_empty(),
            "Rust: -logtostderr writes no file",
        )
    }
}
