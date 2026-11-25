//! Where the lines go against glog's (`testdata/golden/sinks.json`, which `testdata/go/sinks`
//! writes): stderr and the log file under every flag combination of the golden
//! (`logging.stderr-threshold`, restored), and the file's name, place and layout
//! (`logging.files`).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::{ExitCode, Termination as _},
    };

    use pbs_logger::{fatalf, parse_command_line, subscriber};
    use serde_json::Value;
    use tracing::subscriber::with_default;

    use super::support::{
        Buffer, Scratch, TestResult, Written, check, check_eq, glog_lines, golden, list,
        log_levels, sinks_run, strings, text, written,
    };

    /// The directories of one run, as the sinks golden's runs have them: `TMPDIR`, the
    /// `-log_dir` and `-log_link` directories, and a path that does not exist.
    struct Dirs {
        _root: Scratch,
        temp: PathBuf,
        logs: PathBuf,
        links: PathBuf,
        missing: PathBuf,
    }

    impl Dirs {
        fn new() -> TestResult<Self> {
            let root = Scratch::new()?;
            let dirs = Self {
                temp: root.path().join("tmp"),
                logs: root.path().join("logs"),
                links: root.path().join("links"),
                missing: root.path().join("missing"),
                _root: root,
            };
            for dir in [&dirs.temp, &dirs.logs, &dirs.links] {
                fs::create_dir_all(dir)?;
            }
            Ok(dirs)
        }

        /// `arg` with the golden's placeholders replaced by these directories.
        fn replace(&self, arg: &str) -> String {
            let mut arg = arg.to_owned();
            for (placeholder, dir) in [
                ("LOGDIR", &self.logs),
                ("LINKDIR", &self.links),
                ("MISSING", &self.missing),
            ] {
                arg = arg.replace(placeholder, &dir.to_string_lossy());
            }
            arg
        }

        /// The names in `dir`, none when it does not exist.
        fn names(dir: &Path) -> TestResult<Vec<String>> {
            if dir.exists() {
                Scratch::names(dir)
            } else {
                Ok(Vec::new())
            }
        }
    }

    /// What one Rust run of a sinks golden run did.
    struct Ran {
        dirs: Dirs,
        stderr: Vec<Written>,
        exit: Option<ExitCode>,
    }

    impl Ran {
        /// The lines of every file the run made in `dir`.
        fn file_lines(dir: &Path) -> TestResult<Vec<Written>> {
            let mut lines = Vec::new();
            for name in Dirs::names(dir)? {
                lines.extend(written(&fs::read_to_string(dir.join(name))?)?);
            }
            Ok(lines)
        }
    }

    /// Runs the scenario of `run` (the four facade levels, or `Fatalf` reported as `main` reports
    /// it) under its arguments, with the program named `prebid-server` and the temp directory as
    /// the default log directory.
    fn run_like(run: &Value) -> TestResult<Ran> {
        let dirs = Dirs::new()?;
        let mut args = vec!["prebid-server".into()];
        args.extend(
            strings(run, "/args")?
                .iter()
                .map(|arg| dirs.replace(arg).into()),
        );
        let mut usage = Buffer::default();
        let flags = parse_command_line(&args, &mut usage)
            .map_err(|exit| format!("{args:?}: {exit:?}: {}", usage.text()))?
            .flags;
        let stderr = Buffer::default();
        let (built, logging) = subscriber(&flags, "prebid-server", &dirs.temp, stderr.clone())?;
        let exit = with_default(built, || match text(run, "/scenario") {
            Ok("fatal") => Ok(Some(fatalf!("fatalf {}", 5).report())),
            Ok("levels") => {
                log_levels();
                Ok(None)
            }
            other => Err(format!("no Rust run for the {other:?} scenario")),
        })?;
        drop(logging);
        Ok(Ran {
            dirs,
            stderr: written(&stderr.text())?,
            exit,
        })
    }

    /// The glog lines of the golden file of `run` whose name has `.log.<severity>.`.
    fn go_file(run: &Value, severity: &str) -> TestResult<Vec<(String, String)>> {
        let infix = format!(".log.{severity}.");
        let file = list(run, "/files")?
            .iter()
            .find(|file| text(file, "/name").is_ok_and(|name| name.contains(&infix)))
            .ok_or_else(|| format!("no {severity} file"))?;
        glog_lines(file, "/lines")
    }

    /// Every levels and fatal run of the golden: the lines on stderr are glog's, in order and
    /// with glog's severities, under `-logtostderr` (every line, no file), `-alsologtostderr`
    /// (every line and the file) and otherwise `-stderrthreshold` (the lines at or above it, the
    /// file every line; `Fatalf`'s at every threshold), and a file is written when glog writes
    /// one, holding the lines of glog's INFO file. glog's stacks and `SIGABRT` line after
    /// `Fatalf` are `logging.fatal`, its stack at `-log_backtrace_at` is `logging.v-flags`.
    // Go: glog v1.2.5, glog_file.go:198-236 (stderrSink.Enabled and fileSink.Enabled) and
    // glog_flags.go:388-397; main.go:28 parses the flags
    #[test]
    fn stderr_threshold() -> TestResult {
        let sinks = golden("sinks.json")?;
        let mut compared = 0;
        for run in list(&sinks, "/runs")? {
            let scenario = text(run, "/scenario")?;
            if scenario != "levels" && scenario != "fatal" {
                continue;
            }
            let args = strings(run, "/args")?;
            let ran = run_like(run)?;
            let rust: Vec<(String, String)> = ran.stderr.iter().map(Written::glog_like).collect();
            check_eq(
                &rust,
                &glog_lines(run, "/stderr")?,
                &format!("{scenario} {args:?}: stderr"),
            )?;
            let mut files = Ran::file_lines(&ran.dirs.temp)?;
            files.extend(Ran::file_lines(&ran.dirs.logs)?);
            files.extend(Ran::file_lines(&ran.dirs.missing)?);
            let go_dir = text(run, "/dir")?;
            if go_dir.is_empty() {
                check(
                    files.is_empty(),
                    &format!("{scenario} {args:?}: no file, as glog writes none"),
                )?;
            } else {
                let rust_file: Vec<(String, String)> =
                    files.iter().map(Written::glog_like).collect();
                check_eq(
                    &rust_file,
                    &go_file(run, "INFO")?,
                    &format!("{scenario} {args:?}: the file holds glog's INFO file's lines"),
                )?;
            }
            let go_exit = run.pointer("/exit").and_then(Value::as_u64);
            match ran.exit {
                Some(exit) => {
                    check_eq(&go_exit, &Some(2), "glog exits 2 after Fatalf")?;
                    check_eq(&exit, &ExitCode::from(2), "Fatal reports exit code 2")?;
                }
                None => check_eq(&go_exit, &Some(0), "glog exits 0")?,
            }
            compared += 1;
        }
        check_eq(&compared, &20, "every levels and fatal run is compared")
    }

    /// Whether `name` is tracing-appender's daily file of `prebid-server`:
    /// `prebid-server.<yyyy-mm-dd>.log`.
    fn daily_file(name: &str) -> bool {
        name.strip_prefix("prebid-server.")
            .and_then(|rest| rest.strip_suffix(".log"))
            .is_some_and(|date| {
                date.len() == 10
                    && date.bytes().enumerate().all(|(at, byte)| {
                        if at == 4 || at == 7 {
                            byte == b'-'
                        } else {
                            byte.is_ascii_digit()
                        }
                    })
            })
    }

    /// glog writes one file per severity, `<program>.<host>.<user>.log.<SEVERITY>.<time>.<pid>`
    /// with a five-line header, each holding the lines at or above its severity, a symlink per
    /// severity next to them and another in `-log_link`; when `-log_dir` does not exist it
    /// writes them in the temp directory. tracing-appender writes one file,
    /// `<program>.<yyyy-mm-dd>.log` (rolled over at midnight UTC), with every line and no
    /// header, and no symlink; a `-log_dir` that does not exist is created, and one that cannot
    /// be made fails (and startup with it, exit code 2) where glog falls back to the temp
    /// directory.
    // Go: glog v1.2.5, glog_file.go:38-55 (the directories), 95-111 (logName), 115-165 (create
    // and createInDir) and 330-338 (the header)
    #[test]
    fn appender_layout() -> TestResult {
        let sinks = golden("sinks.json")?;
        let linked = sinks_run(&sinks, "levels", &["-log_dir=LOGDIR", "-log_link=LINKDIR"])?;
        let missing = sinks_run(&sinks, "levels", &["-log_dir=MISSING"])?;
        go_layout(linked, missing)?;
        rust_layout(linked, missing)?;
        unusable_log_dir()
    }

    /// Go's side of [`appender_layout`]: glog's files, symlinks and fallback in the golden.
    fn go_layout(linked: &Value, missing: &Value) -> TestResult {
        check_eq(
            text(linked, "/dir")?,
            "log_dir",
            "Go: the files are in -log_dir",
        )?;
        let names = |pointer: &str| -> TestResult<Vec<String>> {
            list(linked, pointer)?
                .iter()
                .map(|file| text(file, "/name").map(str::to_owned))
                .collect()
        };
        check_eq(
            names("/files")?.as_slice(),
            &[
                "prebid-server.ERROR",
                "prebid-server.HOST.USER.log.ERROR.YYYYMMDD-HHMMSS.PID",
                "prebid-server.HOST.USER.log.INFO.YYYYMMDD-HHMMSS.PID",
                "prebid-server.HOST.USER.log.WARNING.YYYYMMDD-HHMMSS.PID",
                "prebid-server.INFO",
                "prebid-server.WARNING",
            ]
            .map(str::to_owned)[..],
            "Go: a file and a symlink per severity",
        )?;
        check_eq(
            names("/links")?.as_slice(),
            &[
                "prebid-server.ERROR",
                "prebid-server.INFO",
                "prebid-server.WARNING",
            ]
            .map(str::to_owned)[..],
            "Go: a symlink per severity in -log_link",
        )?;
        let letters = |severity: &str| -> TestResult<Vec<String>> {
            Ok(go_file(linked, severity)?
                .into_iter()
                .map(|(letter, _)| letter)
                .collect())
        };
        check_eq(
            &(letters("INFO")?, letters("WARNING")?, letters("ERROR")?),
            &(
                ["I", "I", "W", "E"].map(str::to_owned).to_vec(),
                ["W", "E"].map(str::to_owned).to_vec(),
                vec!["E".to_owned()],
            ),
            "Go: each file holds the lines at or above its severity",
        )?;
        check_eq(
            text(missing, "/dir")?,
            "temp",
            "Go: a missing -log_dir falls back to the temp directory",
        )
    }

    /// Rust's side of [`appender_layout`] for the same two runs.
    fn rust_layout(linked: &Value, missing: &Value) -> TestResult {
        let ran = run_like(linked)?;
        let made = Scratch::names(&ran.dirs.logs)?;
        check(
            made.len() == 1 && made.iter().all(|name| daily_file(name)),
            &format!("Rust: one daily file in -log_dir, not {made:?}"),
        )?;
        let lines = Ran::file_lines(&ran.dirs.logs)?;
        check_eq(
            lines
                .iter()
                .map(Written::letter)
                .collect::<Vec<_>>()
                .as_slice(),
            &["I", "I", "W", "E"].map(str::to_owned)[..],
            "Rust: the file holds every line and no header",
        )?;
        check_eq(
            &Scratch::names(&ran.dirs.links)?,
            &Vec::new(),
            "Rust: no symlink in -log_link",
        )?;
        check_eq(
            &Scratch::names(&ran.dirs.temp)?,
            &Vec::new(),
            "Rust: nothing in the temp directory",
        )?;

        let created = run_like(missing)?;
        let in_missing = Dirs::names(&created.dirs.missing)?;
        check(
            in_missing.len() == 1 && in_missing.iter().all(|name| daily_file(name)),
            &format!("Rust: the missing -log_dir is made and holds the file, not {in_missing:?}"),
        )?;
        check_eq(
            &Scratch::names(&created.dirs.temp)?,
            &Vec::new(),
            "Rust: nothing in the temp directory",
        )
    }

    /// A `-log_dir` under a regular file, which no one can make: the subscriber fails.
    fn unusable_log_dir() -> TestResult {
        let dirs = Dirs::new()?;
        let blocker = dirs.logs.join("file");
        fs::write(&blocker, "")?;
        let unusable = blocker.join("logs");
        let mut usage = Buffer::default();
        let arg = format!("-log_dir={}", unusable.display());
        let flags = parse_command_line(&["prebid-server".into(), arg.into()], &mut usage)
            .map_err(|exit| format!("{exit:?}: {}", usage.text()))?
            .flags;
        match subscriber(&flags, "prebid-server", &dirs.temp, Buffer::default()) {
            Ok(_) => Err("Rust: a -log_dir that cannot be made is no fallback".into()),
            Err(err) => check(
                err.to_string().starts_with(&format!(
                    "log: cannot create log in {}: ",
                    unusable.display()
                )),
                &format!("Rust: a -log_dir that cannot be made fails: {err}"),
            ),
        }
    }
}
