//! `init_from_glog_flags`, which `main` calls first: what it writes and returns when the command
//! line asks for the usage, does not parse or names a log directory that cannot be made, the
//! global subscriber it installs, which the `log` crate's records reach too, and the log file it
//! names after the program, and a second call. One test, as a process has one global subscriber.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs, iter,
        process::{ExitCode, Termination as _},
        thread,
    };

    use pbs_logger::{Exit, infof, init_from_glog_flags, warnf};
    use tracing::info;

    use super::support::{Buffer, PREBID, Scratch, TestResult, Written, check, check_eq, written};

    /// The command line of a binary at `/usr/local/bin/prebid-server`.
    fn args(rest: &[&str]) -> Vec<OsString> {
        iter::once("/usr/local/bin/prebid-server")
            .chain(rest.iter().copied())
            .map(OsString::from)
            .collect()
    }

    /// The level and message of each line.
    fn shown(lines: &[Written]) -> Vec<(String, String)> {
        lines
            .iter()
            .map(|line| (line.level.clone(), line.message.clone()))
            .collect()
    }

    /// Logs from another thread and from this one: through the facade, as a dependency through
    /// tracing, and through the `log` crate as a dependency (reqwest) and as Prebid's code
    /// (pbs-currency) do. Returns the level and message of the lines the subscriber keeps.
    fn log_everywhere() -> TestResult<Vec<(String, String)>> {
        thread::spawn(|| infof!(target: PREBID, "from another thread"))
            .join()
            .map_err(|_panic| "the logging thread panicked")?;
        warnf!(target: PREBID, "warnf {}", 3);
        info!(target: "h2::proto::connection", "an INFO line of a dependency");
        log::info!(target: "reqwest::connect", "an INFO line of a dependency through log");
        log::warn!(target: "reqwest::connect", "a WARN line of a dependency through log");
        log::info!(target: "pbs_currency::rates", "an INFO line of Prebid's through log");
        log::debug!(target: "pbs_currency::rates", "a DEBUG line of Prebid's through log");
        Ok([
            ("INFO", "from another thread"),
            ("WARN", "warnf 3"),
            ("WARN", "a WARN line of a dependency through log"),
            ("INFO", "an INFO line of Prebid's through log"),
        ]
        .map(|(level, message)| (level.to_owned(), message.to_owned()))
        .to_vec())
    }

    /// `flag.Parse` with `flag.ExitOnError` writes the usage and exits 0 on `-help`, and writes
    /// the error line and the usage and exits 2 on a bad flag; the usage names the program as
    /// `os.Args[0]` gives it. glog names its files after the program's base name. Here the
    /// failures come back as [`Exit`], whose report is the exit code; a log directory that
    /// cannot be made stops startup with exit code 2 (`logging.files`); the subscriber becomes
    /// every thread's and takes the records of crates that log through the `log` crate, as
    /// reqwest and pbs-currency do, under the same filter (`logging.std-log-lines`); the log file
    /// is named after the program's base name, and a second start fails with exit code 2.
    // Go: main.go:28; flag/flag.go:684-691 (defaultUsage) and 1153-1177 (Parse); glog v1.2.5,
    // glog_file.go:57-62 (the program name)
    #[test]
    fn init_once() -> TestResult {
        let scratch = Scratch::new()?;
        let help = Buffer::default();
        let help_exit = init_from_glog_flags(&args(&["-help"]), scratch.path(), help.clone()).err();
        check_eq(&help_exit, &Some(Exit::Help), "-help")?;
        check_eq(
            &help_exit.map(Exit::report),
            &Some(ExitCode::from(0)),
            "-help exits 0",
        )?;
        check(
            help.text()
                .starts_with("Usage of /usr/local/bin/prebid-server:\n\nOptions:\n"),
            &format!("the usage names the program: {}", help.text()),
        )?;

        let bad = Buffer::default();
        let bad_exit = init_from_glog_flags(&args(&["-nope"]), scratch.path(), bad.clone()).err();
        check_eq(&bad_exit, &Some(Exit::Error), "-nope")?;
        check_eq(
            &bad_exit.map(Exit::report),
            &Some(ExitCode::from(2)),
            "-nope exits 2",
        )?;
        check_eq(
            &bad.text(),
            &format!("flag provided but not defined: -nope\n{}", help.text()),
            "the error line, then the usage",
        )?;

        let blocker = scratch.path().join("file");
        fs::write(&blocker, "")?;
        let unusable = blocker.join("logs");
        let no_dir = Buffer::default();
        let no_dir_exit = init_from_glog_flags(
            &args(&[&format!("-log_dir={}", unusable.display())]),
            scratch.path(),
            no_dir.clone(),
        )
        .err();
        check_eq(
            &no_dir_exit,
            &Some(Exit::Error),
            "a -log_dir that cannot be made",
        )?;
        check(
            no_dir.text().starts_with(&format!(
                "log: cannot create log in {}: ",
                unusable.display()
            )),
            &format!("the reason: {}", no_dir.text()),
        )?;

        let logs = scratch.path().join("logs");
        let stderr = Buffer::default();
        let logging = init_from_glog_flags(
            &args(&["-alsologtostderr", &format!("-log_dir={}", logs.display())]),
            scratch.path(),
            stderr.clone(),
        )
        .map_err(|failed| format!("{failed:?}: {}", stderr.text()))?;
        let want = log_everywhere()?;
        drop(logging);
        check_eq(&shown(&written(&stderr.text())?), &want, "stderr")?;
        let names = Scratch::names(&logs)?;
        let [name] = names.as_slice() else {
            return Err(format!("one log file, not {names:?}").into());
        };
        let date = name
            .strip_prefix("prebid-server.")
            .and_then(|rest| rest.strip_suffix(".log"));
        check(
            date.is_some_and(|date| date.len() == 10),
            &format!("the file is named after the program's base name: {name}"),
        )?;
        check_eq(
            &shown(&written(&fs::read_to_string(logs.join(name))?)?),
            &want,
            "the file",
        )?;

        let again = Buffer::default();
        let again_exit =
            init_from_glog_flags(&args(&["-logtostderr"]), scratch.path(), again.clone()).err();
        check_eq(&again_exit, &Some(Exit::Error), "a second start")?;
        check_eq(
            &again.text(),
            &"a global default trace dispatcher has already been set\n".to_owned(),
            "the reason",
        )
    }
}
