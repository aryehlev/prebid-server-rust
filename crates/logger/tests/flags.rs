//! The command line against Go's `flag.Parse` with glog's nine flags (`testdata/golden/flags.json`
//! and `flags_seeded.json`, which `testdata/go/flags` writes): outcome, exit code, values and the
//! arguments left (`logging.flags`, restored), the error lines and usage (`logging.flag-texts`),
//! the flags Prebid Server never uses (`logging.v-flags`) and arguments that are not UTF-8
//! (`logging.flag-utf8`).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fmt::Write as _, os::unix::ffi::OsStringExt as _};

    use pbs_logger::{CommandLine, Exit, GlogFlags, parse_command_line, subscriber};
    use serde_json::Value;
    use sha2::{Digest as _, Sha256};
    use tracing::subscriber::with_default;

    use super::support::{
        Buffer, Scratch, TestResult, Written, check, check_eq, command_line, glog_lines, golden,
        list, log_levels, sinks_run, strings, text, written,
    };

    /// The nine flags' values as Go's `Flag.Value.String()` writes them, in Go's order.
    fn go_values(flags: &GlogFlags) -> [(&'static str, String); 9] {
        fn join<T: ToString>(items: &[T]) -> String {
            items
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        }
        [
            ("alsologtostderr", flags.alsologtostderr.to_string()),
            ("log_backtrace_at", join(&flags.log_backtrace_at)),
            ("log_dir", flags.log_dir.clone()),
            ("log_link", flags.log_link.clone()),
            ("logbuflevel", flags.logbuflevel.to_string()),
            ("logtostderr", flags.logtostderr.to_string()),
            (
                "stderrthreshold",
                flags.stderrthreshold.number().to_string(),
            ),
            ("v", flags.v.to_string()),
            ("vmodule", join(&flags.vmodule)),
        ]
    }

    /// What one command line did on the Rust side.
    struct Parsed {
        result: Result<CommandLine, Exit>,
        stderr: String,
    }

    impl Parsed {
        fn of(args: &[OsString]) -> Self {
            let mut stderr = Buffer::default();
            let result = parse_command_line(args, &mut stderr);
            Self {
                result,
                stderr: stderr.text(),
            }
        }

        /// `parsed`, `help` or `error`, as the golden says.
        const fn outcome(&self) -> &'static str {
            match self.result {
                Ok(_) => "parsed",
                Err(Exit::Help) => "help",
                Err(Exit::Error) => "error",
            }
        }

        /// The line before the usage listing of an error.
        fn error_line(&self) -> Option<&str> {
            self.result
                .as_ref()
                .is_err_and(|exit| *exit == Exit::Error)
                .then(|| self.stderr.lines().next().unwrap_or_default())
        }
    }

    /// The golden row's expectation, compared with what Rust did.
    fn check_row(row: &Value, parsed: &Parsed) -> TestResult {
        let args = strings(row, "/args")?;
        let want = text(row, "/outcome")?;
        if parsed.outcome() != want {
            return Err(format!(
                "{args:?}: {} where Go's flag.Parse gives {want}: {}",
                parsed.outcome(),
                parsed.stderr
            )
            .into());
        }
        match &parsed.result {
            Ok(command_line) => {
                let values = row.pointer("/values").ok_or("no values")?;
                for (name, value) in go_values(&command_line.flags) {
                    let go = text(values, &format!("/{name}"))?;
                    if value != go {
                        return Err(format!("{args:?}: -{name} is {value:?}, Go's {go:?}").into());
                    }
                }
                let rest = if row.pointer("/rest").is_some() {
                    strings(row, "/rest")?
                } else {
                    Vec::new()
                };
                if command_line.args != rest {
                    return Err(format!(
                        "{args:?}: arguments left {:?}, Go's {rest:?}",
                        command_line.args
                    )
                    .into());
                }
            }
            Err(exit) => {
                let go = row.pointer("/exit").and_then(Value::as_u64);
                if go != Some(u64::from(exit.code())) {
                    return Err(format!("{args:?}: exit {}, Go's {go:?}", exit.code()).into());
                }
            }
        }
        Ok(())
    }

    /// Every row of the matrix: the outcome (parsed, help or error), the exit code of help (0)
    /// and of an error (2), and, when the command line parses, every flag's value and the
    /// arguments left, as Go's `flag.Parse` gives them with glog's flags.
    // Go: main.go:28 (flag.Parse); flag/flag.go:1075-1147 (parseOne); glog v1.2.5,
    // glog_flags.go:101-398 and glog_file.go:44-47 (the flags and their Set methods)
    #[test]
    fn go_flag_matrix() -> TestResult {
        let doc = golden("flags.json")?;
        let rows = list(&doc, "/rows")?;
        check(rows.len() > 100, "the matrix has its rows")?;
        for row in rows {
            let args = strings(row, "/args")?;
            check_row(row, &Parsed::of(&command_line(&args)))?;
        }
        Ok(())
    }

    /// Go's `splitmix64` (`testdata/go/flags/seeded.go`).
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: usize) -> TestResult<usize> {
            Ok(usize::try_from(self.next_u64() % u64::try_from(n)?)?)
        }

        fn pick<'p>(&mut self, pool: &'p [String]) -> TestResult<&'p str> {
            let at = self.below(pool.len())?;
            Ok(pool.get(at).ok_or("empty pool")?)
        }
    }

    /// The pools of the seeded golden.
    struct Pools {
        names: Vec<String>,
        odd_names: Vec<String>,
        values: Vec<String>,
        specials: Vec<String>,
    }

    impl Pools {
        /// Go's `commandLine`: the steps `seeded.go` documents, in its order.
        fn command_line(&self, rng: &mut SplitMix64) -> TestResult<Vec<String>> {
            let mut args = Vec::new();
            for _ in 0..rng.below(7)? {
                let k = rng.below(10)?;
                if k < 6 {
                    let dash = match rng.below(8)? {
                        0..5 => "-",
                        5..7 => "--",
                        _ => "---",
                    };
                    let name = if rng.below(4)? < 3 {
                        rng.pick(&self.names)?
                    } else {
                        rng.pick(&self.odd_names)?
                    };
                    let mut arg = format!("{dash}{name}");
                    if rng.below(3)? == 0 {
                        write!(arg, "={}", rng.pick(&self.values)?)?;
                    }
                    args.push(arg);
                } else if k < 9 {
                    args.push(rng.pick(&self.values)?.to_owned());
                } else {
                    args.push(rng.pick(&self.specials)?.to_owned());
                }
            }
            Ok(args)
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap_or_default();
            out
        })
    }

    /// Go's `encodeList`: the count, `:`, the items hex-encoded and joined by `,`.
    fn encode_list<S: AsRef<str>>(items: &[S]) -> String {
        let encoded: Vec<String> = items.iter().map(|s| hex(s.as_ref().as_bytes())).collect();
        format!("{}:{}", items.len(), encoded.join(","))
    }

    /// Go's `encodeResult`.
    fn encode_result(parsed: &Parsed) -> String {
        match &parsed.result {
            Ok(command_line) => {
                let values: Vec<String> = go_values(&command_line.flags)
                    .iter()
                    .map(|(_, value)| hex(value.as_bytes()))
                    .collect();
                format!(
                    "parsed {} {}\n",
                    values.join(","),
                    encode_list(&command_line.args)
                )
            }
            Err(Exit::Help) => "help\n".to_owned(),
            Err(Exit::Error) => "error\n".to_owned(),
        }
    }

    /// The seeded corpus, its digests and the outcome counts.
    struct Seeded {
        doc: Value,
        pools: Pools,
    }

    impl Seeded {
        fn load() -> TestResult<Self> {
            let doc = golden("flags_seeded.json")?;
            let pools = Pools {
                names: strings(&doc, "/pools/names")?,
                odd_names: strings(&doc, "/pools/odd_names")?,
                values: strings(&doc, "/pools/values")?,
                specials: strings(&doc, "/pools/specials")?,
            };
            Ok(Self { doc, pools })
        }

        /// Rebuilds every command line, checks the inputs' digests and returns, per batch, the
        /// digests `digest` makes of each parse.
        fn check<F: Fn(&Parsed) -> String>(&self, field: &str, digest: F) -> TestResult<usize> {
            let seed = self.doc.pointer("/seed").and_then(Value::as_u64);
            let count = self.doc.pointer("/count").and_then(Value::as_u64);
            let batch = self.doc.pointer("/batch").and_then(Value::as_u64);
            let (Some(seed), Some(count), Some(batch)) = (seed, count, batch) else {
                return Err("the seeded golden has no seed, count or batch".into());
            };
            let batches = list(&self.doc, "/batches")?;
            let mut rng = SplitMix64(seed);
            let (mut inputs, mut results) = (Sha256::new(), Sha256::new());
            let mut checked = 0;
            for i in 1..=count {
                let args = self.pools.command_line(&mut rng)?;
                inputs.update(format!("{}\n", encode_list(&args)));
                results.update(digest(&Parsed::of(&command_line(&args))));
                if i % batch == 0 {
                    let at = usize::try_from(i / batch - 1)?;
                    let want = batches.get(at).ok_or("a batch is missing")?;
                    let got_inputs = hex(&inputs.finalize_reset());
                    if got_inputs != text(want, "/inputs_sha256")? {
                        return Err(
                            format!("batch {at}: the command lines differ from Go's").into()
                        );
                    }
                    if hex(&results.finalize_reset()) != text(want, field)? {
                        return Err(format!("batch {at}: {field} differs from Go's").into());
                    }
                    checked += 1;
                }
            }
            Ok(checked)
        }
    }

    /// 50,000 seeded command lines: the outcome, every value and the arguments left of each are
    /// Go's (per-batch SHA-256 digests of `flags_seeded.json`).
    // Go: testdata/go/flags/seeded.go, the emulation `go run ./flags` checks against flag.Parse
    #[test]
    fn seeded_command_lines_match_go() -> TestResult {
        let seeded = Seeded::load()?;
        let batches = seeded.check("/results_sha256", encode_result)?;
        check_eq(&batches, &50, "every batch of the corpus is compared")
    }

    /// What `-h` writes: getopts' usage listing of glog's flags, with glog's help texts.
    const GETOPTS_USAGE: &str = "\
Usage of prebid-server:

Options:
        -alsologtostderr [=bool]
                        log to standard error as well as files
        -log_backtrace_at value
                        when logging hits line file:N, emit a stack trace
        -log_dir string If non-empty, write log files in this directory
        -log_link string
                        If non-empty, add symbolic links in this directory to
                        the log files
        -logbuflevel int
                        Buffer log messages logged at this level or lower (-1
                        means don't buffer; 0 means buffer INFO only; ...).
                        Has limited applicability on non-prod platforms.
        -logtostderr [=bool]
                        log to standard error instead of files
        -stderrthreshold value
                        logs at or above this threshold go to stderr
    -v value            log level for V logs
        -vmodule value  comma-separated list of pattern=N settings for
                        file-filtered logging
";

    /// Go's flag package writes an error line and then its usage listing; the error lines are
    /// Go's words, with values quoted as Rust quotes strings, which differs from Go's `%q` for
    /// control characters (the two rows below); the usage listing is getopts' layout with glog's
    /// help texts.
    // Go: flag/flag.go:1058-1062 (failf), 1075-1147 (parseOne's error lines) and 607-651
    // (PrintDefaults)
    #[test]
    fn error_lines() -> TestResult {
        let doc = golden("flags.json")?;
        let quoted_differently = [
            (
                "invalid value \"\\x01\" for flag -v: strconv.Atoi: parsing \"\\x01\": invalid \
                 syntax",
                "invalid value \"\\u{1}\" for flag -v: strconv.Atoi: parsing \"\\u{1}\": invalid \
                 syntax",
            ),
            (
                "invalid boolean value \"\\x7f\" for -logtostderr: parse error",
                "invalid boolean value \"\\u{7f}\" for -logtostderr: parse error",
            ),
        ];
        let mut differing = Vec::new();
        for row in list(&doc, "/rows")? {
            let args = strings(row, "/args")?;
            let parsed = Parsed::of(&command_line(&args));
            let Some(line) = parsed.error_line() else {
                continue;
            };
            let go = text(row, "/error_line")?;
            if line != go {
                differing.push((go.to_owned(), line.to_owned()));
            }
        }
        let want: Vec<(String, String)> = quoted_differently
            .iter()
            .map(|&(go, rust)| (go.to_owned(), rust.to_owned()))
            .collect();
        check_eq(
            &differing,
            &want,
            "only control characters are quoted differently",
        )?;
        let seeded = Seeded::load()?;
        let batches = seeded.check("/error_lines_sha256", |parsed| {
            format!(
                "{}\n",
                hex(parsed.error_line().unwrap_or_default().as_bytes())
            )
        })?;
        check_eq(&batches, &50, "every seeded error line is Go's")?;

        let go_usage = text(&doc, "/usage")?;
        let help = Parsed::of(&command_line(&["-h"]));
        check(
            go_usage.starts_with("Usage of prebid-server:\n  -alsologtostderr\n"),
            "Go's listing",
        )?;
        check_eq(
            help.stderr.as_str(),
            GETOPTS_USAGE,
            "getopts' listing with glog's help texts",
        )?;
        let failed = Parsed::of(&command_line(&["-nope"]));
        check_eq(
            &failed.stderr,
            &format!("flag provided but not defined: -nope\n{}", help.stderr),
            "an error writes its line, then the listing",
        )
    }

    /// `-v` and `-vmodule` are read as glog reads them (the matrix rows) and change nothing, as
    /// Prebid Server calls no `glog.V`; `-log_backtrace_at=logger.go:17` makes glog write a stack
    /// after every `Warnf` line, as the facade's `Warnf` is logger.go:17, and Rust writes none.
    // Go: glog v1.2.5, glog_flags.go:101-186 and 230-331; glog.go:219 (the stack at a backtrace
    // location)
    #[test]
    fn v_flags_accepted() -> TestResult {
        let sinks = golden("sinks.json")?;
        let traced = sinks_run(
            &sinks,
            "levels",
            &["-log_backtrace_at=logger.go:17", "-logtostderr"],
        )?;
        let kinds: Vec<String> = list(traced, "/stderr")?
            .iter()
            .map(|entry| text(entry, "/kind").map(str::to_owned))
            .collect::<TestResult<_>>()?;
        check_eq(
            kinds.as_slice(),
            &["glog", "glog", "glog", "stack", "glog"].map(str::to_owned)[..],
            "Go writes a stack after the Warnf line",
        )?;
        let plain = sinks_run(&sinks, "levels", &["-logtostderr"])?;
        let verbose = sinks_run(
            &sinks,
            "levels",
            &["-v=4", "-vmodule=logger=3,glog=2", "-logtostderr"],
        )?;
        check_eq(
            &glog_lines(verbose, "/stderr")?,
            &glog_lines(plain, "/stderr")?,
            "-v and -vmodule change no line",
        )?;
        for args in [
            vec!["-log_backtrace_at=logger.go:17", "-logtostderr"],
            vec!["-v=4", "-vmodule=logger=3,glog=2", "-logtostderr"],
        ] {
            let parsed = Parsed::of(&command_line(&args));
            let flags = parsed
                .result
                .map_err(|exit| format!("{args:?}: {exit:?}"))?
                .flags;
            let scratch = Scratch::new()?;
            let stderr = Buffer::default();
            let (built, logging) =
                subscriber(&flags, "prebid-server", scratch.path(), stderr.clone())?;
            with_default(built, log_levels);
            drop(logging);
            let lines: Vec<(String, String)> = written(&stderr.text())?
                .iter()
                .map(Written::glog_like)
                .collect();
            check_eq(
                &lines,
                &glog_lines(plain, "/stderr")?,
                &format!("{args:?}: the plain lines, no stack"),
            )?;
            check(
                Scratch::names(scratch.path())?.is_empty(),
                "-logtostderr writes no file",
            )?;
        }
        Ok(())
    }

    /// An argument that is not UTF-8: Go reads it as bytes (a `-log_dir` value or an argument
    /// left over), getopts takes UTF-8 only, so the command line fails with exit code 2.
    // Go: flag/flag.go:1075-1147 works on Go strings, which hold any bytes
    #[test]
    fn non_utf8_arguments() -> TestResult {
        let doc = golden("flags.json")?;
        let mut outcomes = Vec::new();
        for row in list(&doc, "/bytes_rows")? {
            let mut args = vec![OsString::from("prebid-server")];
            for arg in strings(row, "/args")? {
                let bytes = (0..arg.len())
                    .step_by(2)
                    .map(|at| {
                        let pair = arg.get(at..at + 2).ok_or("odd hex")?;
                        Ok(u8::from_str_radix(pair, 16)?)
                    })
                    .collect::<TestResult<Vec<u8>>>()?;
                args.push(OsString::from_vec(bytes));
            }
            let parsed = Parsed::of(&args);
            outcomes.push((
                text(row, "/outcome")?.to_owned(),
                parsed.outcome(),
                parsed.error_line().map(str::to_owned),
            ));
        }
        check_eq(
            outcomes.as_slice(),
            &[
                (
                    "parsed".to_owned(),
                    "error",
                    Some("argument is not UTF-8: -log_dir=\u{FFFD}".to_owned()),
                ),
                (
                    "parsed".to_owned(),
                    "error",
                    Some("argument is not UTF-8: x\u{FFFD}".to_owned()),
                ),
                (
                    "error".to_owned(),
                    "error",
                    Some("argument is not UTF-8: -v=\u{FFFD}".to_owned()),
                ),
            ][..],
            "Go's outcome, Rust's outcome and error line",
        )
    }
}
