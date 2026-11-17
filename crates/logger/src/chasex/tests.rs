//! chasex tests: the files and lines Go's chasex/glog writes, replayed from goldens, and
//! rotation.

use std::{
    error::Error,
    fs,
    path::Path,
    sync::atomic::{AtomicI64, Ordering},
};

use pbs_gocompat::{fmt::Value, time::Time};

use super::{L_DATE, L_LEVEL, L_STD_FLAGS, Level, LogOptions, Logger, RotateMode};
use crate::testutil::{TempDir, TestResult, check, check_eq, fields, fixed_time, read_golden};

/// Go generator's `writeChasex`: the calls every golden case makes, in order.
fn write_chasex(l: &Logger) {
    let mixed = [
        Value::from("a"),
        Value::Int(1),
        Value::Int(2),
        Value::from("b"),
    ];
    l.debug(&[Value::from("hello world")]);
    l.debugf("hello, %s", &[Value::from("chasex")]);
    l.debugln(&mixed);
    l.debug(&mixed);
    l.info(&[Value::from("info message")]);
    l.infof("%d%%", &[Value::Int(50)]);
    l.infoln(&[]);
    l.warn(&[Value::from("warn")]);
    l.warnf("", &[]);
    l.warnln(&[Value::from("x")]);
    l.error(&[Value::from("err\n")]);
    l.errorf("multi\nline", &[]);
    l.errorln(&[Value::Float64(3.5)]);
    l.debug(&[Value::from(r#"{"Request":{"id":"some-id"},"Status":200}"#)]);
}

/// The generator's normalization of [`fixed_time`] in file names.
fn normalize_name(name: &str) -> String {
    name.replace("-20090123-012323", "-YYYYMMDD-HHMMSS")
        .replace("-20090123-01", "-YYYYMMDD-HH")
        .replace("-20090123", "-YYYYMMDD")
}

/// The generator's normalization of [`fixed_time`] and `file.rs:N ` sites in lines.
fn normalize_content(s: &str) -> String {
    let s = s
        .replace("2009/01/23", "DATE")
        .replace("01:23:23.123123", "TIME.MICRO")
        .replace("01:23:23", "TIME");
    s.split_inclusive('\n')
        .map(|line| {
            line.find(".rs:").map_or_else(
                || line.to_owned(),
                |i| {
                    let start = line
                        .get(..i)
                        .and_then(|head| {
                            head.rfind(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        })
                        .map_or(0, |j| j + 1);
                    let after = line.get(i + 4..).unwrap_or_default();
                    let digits =
                        after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                    format!(
                        "{}SITE {}",
                        line.get(..start).unwrap_or_default(),
                        after.get(digits + 1..).unwrap_or_default()
                    )
                },
            )
        })
        .collect()
}

fn mode(name: &str) -> Result<RotateMode, Box<dyn Error>> {
    Ok(match name {
        "none" => RotateMode::None,
        "size" => RotateMode::Size,
        "hour" => RotateMode::Hour,
        "day" => RotateMode::Day,
        _ => return Err(format!("bad mode {name}").into()),
    })
}

fn level(n: &str) -> Result<Level, Box<dyn Error>> {
    let n: i64 = n.parse()?;
    [
        Level::Debug,
        Level::Info,
        Level::Warn,
        Level::Error,
        Level::Fatal,
        Level::Panic,
    ]
    .into_iter()
    .find(|l| l.as_int() == n)
    .ok_or_else(|| format!("bad level {n}").into())
}

/// The sorted names and contents of a directory's files.
fn read_dir_files(dir: &Path) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        files.push((
            entry.file_name().to_string_lossy().into_owned(),
            fs::read_to_string(entry.path())?,
        ));
    }
    files.sort();
    Ok(files)
}

#[test]
fn chasex_golden() -> TestResult {
    // Go: chasex/glog's prefixes for each flag set, level filtering, newline handling, Sprint
    // and Sprintln operand spacing, and the file name suffix of each rotation mode.
    let rows = read_golden("chasex.txt")?;
    check(rows.len() >= 8, "chasex.txt is complete")?;
    for row in &rows {
        let [name, flag, level_n, mode_name, want_file, want_content] = fields::<6>(row)?;
        let dir = TempDir::new("chasex")?;
        let options = LogOptions {
            file: dir.path().join("x.log").display().to_string(),
            flag: flag.parse()?,
            level: level(level_n)?,
            mode: mode(mode_name)?,
            maxsize: 1 << 20,
        };
        let logger = Logger::with_clock(options, fixed_time)?;
        write_chasex(&logger);
        logger.flush();
        let files = read_dir_files(dir.path())?;
        let [(file, content)] = files.as_slice() else {
            return Err(format!("{name}: files {files:?}").into());
        };
        check_eq(normalize_name(file).as_str(), want_file, name)?;
        check_eq(normalize_content(content).as_str(), want_content, name)?;
    }
    Ok(())
}

#[test]
fn chasex_open_errors_golden() -> TestResult {
    // Go: the *os.PathError texts of glog.New.
    let dir = TempDir::new("chasex-errors")?;
    let dir_text = dir.path().display().to_string();
    for row in &read_golden("chasex_errors.txt")? {
        let [file, mode_name, want] = fields::<3>(row)?;
        let options = LogOptions {
            file: file.replace("DIR", &dir_text),
            flag: L_STD_FLAGS,
            level: Level::Debug,
            mode: mode(mode_name)?,
            maxsize: 0,
        };
        let got = Logger::with_clock(options, fixed_time)
            .err()
            .map_or_else(String::new, |e| e.to_string())
            .replace(&dir_text, "DIR")
            .replace("-20090123:", "-YYYYMMDD:");
        check_eq(got.as_str(), want, file)?;
    }
    Ok(())
}

static CLOCK: AtomicI64 = AtomicI64::new(0);

/// A clock whose seconds since 2009-01-23 00:59:59 UTC the test sets.
fn settable_clock() -> Time {
    Time::from_unix(1_232_672_399 + CLOCK.load(Ordering::Relaxed), 0)
}

#[test]
fn rotation_by_size_hour_and_day() -> TestResult {
    let dir = TempDir::new("chasex-rotate")?;
    let base = dir.path().join("r.log").display().to_string();
    let open = |mode, maxsize| {
        Logger::with_clock(
            LogOptions {
                file: format!("{base}-{mode:?}"),
                flag: L_LEVEL,
                level: Level::Debug,
                mode,
                maxsize,
            },
            settable_clock,
        )
    };
    CLOCK.store(0, Ordering::Relaxed);
    let by_size = open(RotateMode::Size, 20)?;
    let by_hour = open(RotateMode::Hour, 0)?;
    let by_day = open(RotateMode::Day, 0)?;
    for l in [&by_size, &by_hour, &by_day] {
        l.debug(&[Value::from("first")]);
    }
    // One second later: past the size limit (a new second, so a new file name); the next hour.
    CLOCK.store(1, Ordering::Relaxed);
    for l in [&by_size, &by_hour, &by_day] {
        l.debug(&[Value::from("second line")]);
    }
    // The next day.
    CLOCK.store(23 * 3600 + 1, Ordering::Relaxed);
    for l in [&by_size, &by_hour, &by_day] {
        l.debug(&[Value::from("third")]);
        l.flush();
    }
    let names: Vec<String> = read_dir_files(dir.path())?
        .into_iter()
        .map(|(name, content)| format!("{} {content:?}", name.replacen("r.log", "", 1)))
        .collect();
    check_eq(
        &names,
        &vec![
            "-Day-20090123 \"DEBUG first\\nDEBUG second line\\n\"".to_owned(),
            "-Day-20090124 \"DEBUG third\\n\"".to_owned(),
            "-Hour-20090123-00 \"DEBUG first\\n\"".to_owned(),
            "-Hour-20090123-01 \"DEBUG second line\\n\"".to_owned(),
            "-Hour-20090124-00 \"DEBUG third\\n\"".to_owned(),
            "-Size-20090123-005959 \"DEBUG first\\n\"".to_owned(),
            "-Size-20090123-010000 \"DEBUG second line\\n\"".to_owned(),
            "-Size-20090124-000000 \"DEBUG third\\n\"".to_owned(),
        ],
        "rotated files",
    )
}

#[test]
fn date_only_prefix_and_levels() -> TestResult {
    let dir = TempDir::new("chasex-date")?;
    let logger = Logger::with_clock(
        LogOptions {
            file: dir.path().join("d.log").display().to_string(),
            flag: L_DATE,
            level: Level::Info,
            mode: RotateMode::None,
            maxsize: 0,
        },
        fixed_time,
    )?;
    logger.debug(&[Value::from("dropped")]);
    logger.infof("kept %d", &[Value::Int(1)]);
    logger.flush();
    check_eq(
        fs::read_to_string(dir.path().join("d.log"))?.as_str(),
        "2009/01/23 kept 1\n",
        "date only",
    )?;
    check_eq(Level::Panic.as_str(), "PANIC", "level names")?;
    check(
        Level::Debug < Level::Info && Level::Fatal < Level::Panic,
        "level order",
    )
}
