//! Go: `glog_file.go`: the per-severity log files.

use std::{
    env::consts,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
};

use pbs_gocompat::time::{Duration, Time};

use super::{Env, Severity};
use crate::goerr;

/// Go: `bufferSize`.
const BUFFER_SIZE: usize = 256 * 1024;

/// Go: `footer`, written after `Next log: name` when a file rotates.
const FOOTER: &str = "\nCONTINUED IN NEXT FILE\n";

/// What creating and rotating files reads (Go: the flags and package variables).
#[derive(Clone, Copy, Debug)]
pub(super) struct FileContext<'a> {
    pub(super) env: &'a Env,
    pub(super) log_dir: &'a str,
    pub(super) log_link: &'a str,
    pub(super) max_size: u64,
}

/// Go: `fileSink`.
#[derive(Debug, Default)]
pub(super) struct FileSink {
    /// One file per severity, by [`Severity::index`].
    files: [Option<SyncBuffer>; 4],
    /// Go: `logDirs`, fixed when the first file is created.
    log_dirs: Option<Vec<PathBuf>>,
}

/// Go: `syncBuffer`: a buffered log file for one severity.
#[derive(Debug)]
struct SyncBuffer {
    severity: Severity,
    writer: Option<BufWriter<File>>,
    /// The current file's path (Go: `file.Name()`).
    path: Option<PathBuf>,
    nbytes: u64,
    made_at: Time,
}

/// Go: `GOOS`/`GOARCH` names for the build target, for the file header.
fn go_platform() -> String {
    let os = match consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    format!("{os}/{arch}")
}

/// Go: `logName`: the file name and the link name for a severity tag.
fn log_name(env: &Env, tag: &str, t: &Time) -> (String, String) {
    let name = format!(
        "{}.{}.{}.log.{tag}.{:04}{:02}{:02}-{:02}{:02}{:02}.{}",
        env.program,
        env.host,
        env.user,
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        env.pid,
    );
    let link = format!("{}.{tag}", env.program);
    (name, link)
}

/// Go: `os.Remove(link); os.Symlink(target, link)`, ignoring errors as Go does.
#[cfg(unix)]
fn replace_link(target: &Path, link: &Path) {
    use std::os::unix::fs::symlink;

    let _removed = fs::remove_file(link);
    let _linked = symlink(target, link);
}

/// No links off Unix.
#[cfg(not(unix))]
const fn replace_link(_target: &Path, _link: &Path) {}

/// Opens a new file for writing that must not exist (Go: `O_RDWR|O_CREATE|O_EXCL`, 0666).
fn create_new(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    let _options = options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let _mode = options.mode(0o666);
    }
    options
        .open(path)
        .map_err(|e| goerr::path_error("open", path, &e))
}

/// Go: `createInDir`.
fn create_in_dir(
    ctx: &FileContext<'_>,
    dir: &Path,
    tag: &str,
    t: &Time,
) -> Result<(File, PathBuf), String> {
    let (name, link) = log_name(ctx.env, tag, t);
    let path = dir.join(&name);
    let file = create_new(&path)?;
    replace_link(Path::new(&name), &dir.join(&link));
    if !ctx.log_link.is_empty() {
        replace_link(&path, &Path::new(ctx.log_link).join(&link));
    }
    Ok((file, path))
}

/// Go: `create`: a new file in the first log directory that accepts one.
fn create(
    ctx: &FileContext<'_>,
    log_dirs: &mut Option<Vec<PathBuf>>,
    tag: &str,
    t: &Time,
) -> Result<(File, PathBuf), String> {
    let dirs = log_dirs.get_or_insert_with(|| {
        let mut dirs = Vec::with_capacity(2);
        if !ctx.log_dir.is_empty() {
            dirs.push(PathBuf::from(ctx.log_dir));
        }
        dirs.push(ctx.env.temp_dir.clone());
        dirs
    });
    let mut last_err = String::new();
    for dir in dirs.iter() {
        match create_in_dir(ctx, dir, tag, t) {
            Ok(created) => return Ok(created),
            Err(e) => last_err = e,
        }
    }
    Err(format!("log: cannot create log: {last_err}"))
}

impl SyncBuffer {
    fn new(severity: Severity) -> Self {
        Self {
            severity,
            writer: None,
            path: None,
            nbytes: 0,
            made_at: Time::default(),
        }
    }

    /// Go: `syncBuffer.rotateFile`: closes the current file with a pointer to the next and
    /// opens a new one with glog's header.
    fn rotate(
        &mut self,
        ctx: &FileContext<'_>,
        log_dirs: &mut Option<Vec<PathBuf>>,
        now: &Time,
    ) -> Result<(), String> {
        let created = create(ctx, log_dirs, self.severity.as_str(), now);
        self.made_at = now.clone();
        let next_name = created.as_ref().map_or_else(
            |_create_error| String::new(),
            |(_, path)| path.display().to_string(),
        );
        let previous = self.path.take();
        if let Some(writer) = self.writer.take()
            && let Ok(mut old) = writer.into_inner()
        {
            let _written = old.write_all(format!("Next log: {next_name}{FOOTER}").as_bytes());
        }
        self.nbytes = 0;
        let (mut file, path) = created?;
        let header = format!(
            "Log file created at: {}\nRunning on machine: {}\nBinary: Built with rustc for \
             {}\nPrevious log: {}\nLog line format: [IWEF]mmdd hh:mm:ss.uuuuuu threadid \
             file:line] msg\n",
            now.format("2006/01/02 15:04:05"),
            ctx.env.host,
            go_platform(),
            previous.map_or_else(|| "<none>".to_owned(), |p| p.display().to_string()),
        );
        let written = file.write_all(header.as_bytes());
        self.writer = Some(BufWriter::with_capacity(BUFFER_SIZE, file));
        let failed = written
            .as_ref()
            .err()
            .map(|e| goerr::path_error("write", &path, e));
        self.path = Some(path);
        if failed.is_none() {
            self.nbytes = u64::try_from(header.len()).unwrap_or(u64::MAX);
        }
        failed.map_or(Ok(()), Err)
    }

    /// Go: `syncBuffer.Write`: rotates when the file would reach the maximum size (at most
    /// once a second), then buffers `p`.
    fn write(
        &mut self,
        ctx: &FileContext<'_>,
        log_dirs: &mut Option<Vec<PathBuf>>,
        p: &[u8],
    ) -> Result<(), String> {
        let len = u64::try_from(p.len()).unwrap_or(u64::MAX);
        if self.nbytes.saturating_add(len) >= ctx.max_size {
            let now = (ctx.env.clock)();
            if now.after(&self.made_at.add(Duration::SECOND))
                || now.second() != self.made_at.second()
            {
                self.rotate(ctx, log_dirs, &now)?;
            }
        }
        let path = self.path.clone().unwrap_or_default();
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| format!("write {}: file already closed", path.display()))?;
        writer
            .write_all(p)
            .map_err(|e| goerr::path_error("write", &path, &e))?;
        self.nbytes = self.nbytes.saturating_add(len);
        Ok(())
    }

    /// Go: `Flush` then `Sync`.
    fn flush(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _flushed = writer.flush();
            let _synced = writer.get_ref().sync_all();
        }
    }
}

impl FileSink {
    /// Go: `fileSink.createMissingFiles`: the files of INFO up to `up_to`, made at once.
    fn create_missing(&mut self, ctx: &FileContext<'_>, up_to: Severity) -> Result<(), String> {
        let Self { files, log_dirs } = self;
        if files.get(up_to.index()).is_some_and(Option::is_some) {
            return Ok(());
        }
        let now = (ctx.env.clock)();
        for severity in Severity::ALL.into_iter().filter(|s| *s <= up_to) {
            let Some(slot) = files.get_mut(severity.index()) else {
                continue;
            };
            if slot.is_some() {
                continue;
            }
            let mut buffer = SyncBuffer::new(severity);
            buffer.rotate(ctx, log_dirs, &now)?;
            *slot = Some(buffer);
        }
        Ok(())
    }

    /// Go: `fileSink.Emit`: writes the line to the files of its severity and every lower one.
    pub(super) fn emit(
        &mut self,
        ctx: &FileContext<'_>,
        severity: Severity,
        line: &[u8],
    ) -> Result<(), String> {
        self.create_missing(ctx, severity)?;
        let Self { files, log_dirs } = self;
        let mut result = Ok(());
        for s in Severity::ALL.into_iter().rev().filter(|s| *s <= severity) {
            if let Some(Some(buffer)) = files.get_mut(s.index()) {
                let written = buffer.write(ctx, log_dirs, line);
                result = result.and(written);
            }
        }
        result
    }

    /// Go: `fileSink.flush(threshold)`: the files of `threshold` and above.
    pub(super) fn flush(&mut self, threshold: Severity) {
        for s in Severity::ALL.into_iter().rev().filter(|s| *s >= threshold) {
            if let Some(Some(buffer)) = self.files.get_mut(s.index()) {
                buffer.flush();
            }
        }
    }
}
