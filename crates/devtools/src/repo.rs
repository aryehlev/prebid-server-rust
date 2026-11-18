//! The two roots every command works with, the commit the goldens derive from, running Go, and
//! writing or checking generated files.
//!
//! The *workspace* is this Rust repository: goldens and generated sources are written into it.
//! The *Go root* is a checkout of the upstream Go server at the commit [`PIN_FILE`] pins: the Go
//! programs build against it, and its sources and static files are read from it. The Go root is
//! `$PBS_GO_ROOT` when that is set, else [`DEFAULT_GO_ROOT`] at the workspace root, where
//! [`FETCH_SCRIPT`] checks it out ([`go_root`]). It is never searched for: when neither exists,
//! every command and test that needs it fails and says to run the script.

use std::{
    env,
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::error::{Result, ToolError};

/// The module path of the upstream Go server.
pub const GO_MODULE: &str = "github.com/prebid/prebid-server/v4";

/// Directory, relative to the workspace root, that holds the generated goldens.
pub const TESTDATA: &str = "testdata";

/// The file, relative to the workspace root, whose one non-comment line is the upstream commit
/// the port is pinned to.
pub const PIN_FILE: &str = "UPSTREAM_SYNC";

/// The script, relative to the workspace root, that checks out the Go root.
pub const FETCH_SCRIPT: &str = "scripts/fetch-upstream.sh";

/// Where the Go root is by default, relative to the workspace root.
pub const DEFAULT_GO_ROOT: &str = ".upstream";

/// The Go module, relative to the workspace root, that holds the dump programs and the typegen
/// describer.
pub const GO_TOOLS: &str = "crates/devtools/go";

/// The workspace root: the directory two levels above this crate's manifest, which holds the root
/// `Cargo.toml` and [`PIN_FILE`].
///
/// # Errors
///
/// Fails when that directory has no `Cargo.toml` or no [`PIN_FILE`].
pub fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.ancestors().nth(2).ok_or_else(|| {
        ToolError::new(format!(
            "{} has no workspace root above it",
            manifest_dir.display()
        ))
    })?;
    if !root.join("Cargo.toml").is_file() || !root.join(PIN_FILE).is_file() {
        return Err(ToolError::new(format!(
            "{} is not the workspace root (it needs Cargo.toml and {PIN_FILE})",
            root.display()
        )));
    }
    Ok(root.to_path_buf())
}

/// The Go root, absolute and with its symbolic links resolved.
///
/// It is `$PBS_GO_ROOT` when that is set and not empty (a relative path is taken from the
/// workspace root), else [`DEFAULT_GO_ROOT`] at the workspace root.
///
/// # Errors
///
/// Fails, saying to run [`FETCH_SCRIPT`], when the directory does not exist or is not a checkout
/// of [`GO_MODULE`].
pub fn go_root() -> Result<PathBuf> {
    resolve_go_root(&workspace_root()?, None)
}

fn resolve_go_root(workspace: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    let from_env = env::var_os("PBS_GO_ROOT").filter(|v| !v.is_empty());
    let (dir, source) = match (explicit, from_env) {
        (Some(dir), _) => (dir.to_path_buf(), "--go-root"),
        (None, Some(dir)) => (workspace.join(dir), "PBS_GO_ROOT"),
        (None, None) => (workspace.join(DEFAULT_GO_ROOT), "the default"),
    };
    let fetch = if source == "the default" {
        format!("run {FETCH_SCRIPT} to check out the commit {PIN_FILE} pins there")
    } else {
        format!(
            "run {FETCH_SCRIPT} --dir {} to check out the commit {PIN_FILE} pins there",
            dir.display()
        )
    };
    let resolved = fs::canonicalize(&dir).map_err(|e| {
        ToolError::new(format!(
            "the Go root {} ({source}) cannot be used: {e}. It is the upstream Go server that the \
             goldens are generated from and the tests read; {fetch} (or set PBS_GO_ROOT to such \
             a checkout)",
            dir.display()
        ))
    })?;
    if !declares_go_module(&resolved) {
        return Err(ToolError::new(format!(
            "the Go root {} ({source}) is not a checkout of {GO_MODULE}: its go.mod does not \
             declare the module; {fetch}",
            resolved.display()
        )));
    }
    Ok(resolved)
}

fn declares_go_module(dir: &Path) -> bool {
    fs::read_to_string(dir.join("go.mod")).is_ok_and(|s| {
        s.lines()
            .next()
            .is_some_and(|l| l.trim() == format!("module {GO_MODULE}"))
    })
}

/// The commit `workspace`'s [`PIN_FILE`] pins: its one line that is neither empty nor a `#`
/// comment, a full 40-character SHA.
///
/// # Errors
///
/// Fails when the file cannot be read or does not hold exactly one such line.
pub fn pinned_commit(workspace: &Path) -> Result<String> {
    let path = workspace.join(PIN_FILE);
    let text = fs::read_to_string(&path).map_err(|e| ToolError::io(&path, &e))?;
    let pins: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    match pins.as_slice() {
        [pin] if is_full_sha(pin) => Ok((*pin).to_owned()),
        _ => Err(ToolError::new(format!(
            "{} must hold exactly one non-comment line, the full SHA of the pinned upstream commit",
            path.display()
        ))),
    }
}

fn is_full_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The workspace and the Go root a command operates on.
#[derive(Debug, Clone)]
pub struct Repo {
    workspace: PathBuf,
    go_root: PathBuf,
}

impl Repo {
    /// Finds the workspace ([`workspace_root`]) and uses `go_root` as the Go root if given (a
    /// relative path is taken from the current directory), else the one [`go_root`] finds.
    ///
    /// # Errors
    ///
    /// Fails when either root cannot be used; for the Go root, the error says how to fetch it.
    pub fn locate(go_root: Option<&Path>) -> Result<Self> {
        let workspace = workspace_root()?;
        let go_root = resolve_go_root(&workspace, go_root)?;
        Ok(Self { workspace, go_root })
    }

    /// The workspace root.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The Go root.
    #[must_use]
    pub fn go_root(&self) -> &Path {
        &self.go_root
    }

    /// `rel` (slash-separated) joined to the workspace root.
    #[must_use]
    pub fn path(&self, rel: &str) -> PathBuf {
        join(&self.workspace, rel)
    }

    /// `rel` (slash-separated) joined to the Go root.
    #[must_use]
    pub fn go_path(&self, rel: &str) -> PathBuf {
        join(&self.go_root, rel)
    }

    /// The commit [`PIN_FILE`] pins.
    ///
    /// # Errors
    ///
    /// As [`pinned_commit`].
    pub fn pinned_commit(&self) -> Result<String> {
        pinned_commit(&self.workspace)
    }

    /// The commit recorded as `go_commit`: `override_commit`, else `PBS_GO_COMMIT`, else the
    /// pinned commit. The Go root must be a clean checkout of exactly that commit
    /// ([`Repo::verify_go_root`]), so a golden never names a commit it was not made from.
    ///
    /// # Errors
    ///
    /// Fails when the commit is not a full SHA or the Go root is not a clean checkout of it.
    pub fn go_commit(&self, override_commit: Option<&str>) -> Result<String> {
        let commit = match override_commit {
            Some(c) => c.to_owned(),
            None => match env::var("PBS_GO_COMMIT").ok().filter(|c| !c.is_empty()) {
                Some(c) => c,
                None => self.pinned_commit()?,
            },
        };
        if !is_full_sha(&commit) {
            return Err(ToolError::new(format!(
                "go_commit {commit} is not a full 40-character SHA"
            )));
        }
        self.verify_go_root(&commit)?;
        Ok(commit)
    }

    /// Checks that the Go root is a git checkout whose `HEAD` is `commit` and that it has no local
    /// changes or untracked files.
    ///
    /// # Errors
    ///
    /// Fails, saying how to fix the checkout, when it is not one.
    pub fn verify_go_root(&self, commit: &str) -> Result<()> {
        let fetch = format!(
            "run {FETCH_SCRIPT} --dir {} --sha {commit}",
            self.go_root.display()
        );
        let head = self.git_in_go_root(&["rev-parse", "HEAD"]).map_err(|e| {
            ToolError::new(format!(
                "cannot tell which commit the Go root {} is at ({e}); {fetch}",
                self.go_root.display()
            ))
        })?;
        let head = head.trim();
        if head != commit {
            return Err(ToolError::new(format!(
                "the Go root {} is at {head}, not {commit}; {fetch}",
                self.go_root.display()
            )));
        }
        // Without optional locks, `git status` does not refresh the index: the check writes
        // nothing into the Go root.
        let status = self.git_in_go_root(&["--no-optional-locks", "status", "--porcelain"])?;
        if !status.trim().is_empty() {
            return Err(ToolError::new(format!(
                "the Go root {} has local changes or untracked files, so it is not {commit} \
                 itself; clean it or remove it and {fetch}:\n{}",
                self.go_root.display(),
                status.trim_end()
            )));
        }
        Ok(())
    }

    fn git_in_go_root(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.go_root)
            .output()
            .map_err(|e| ToolError::new(format!("running git: {e}")))?;
        if !out.status.success() {
            return Err(ToolError::new(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        String::from_utf8(out.stdout).map_err(|e| ToolError::new(format!("git output: {e}")))
    }

    /// Runs `go <args>` (or `$GO <args>`) in `module`, a workspace-relative Go tooling module,
    /// building against the Go root, with every `PBS_*` variable removed.
    ///
    /// The run uses a temporary `go.work` that holds `module` and replaces [`GO_MODULE`] by the
    /// Go root, which overrides the module's own `replace` (the default Go root), and has the
    /// module's `go` version, so the `GODEBUG` defaults stay those of the upstream module. Its
    /// `go.work.sum` starts as the Go root's `go.sum`: the packages of the Go root that `module`
    /// does not import itself (the adapters `struct-meta` loads) find their checksums there.
    ///
    /// # Errors
    ///
    /// Fails when the module or Go cannot be used, or the command fails (with the tail of its
    /// standard error).
    pub fn go(&self, module: &str, args: &[OsString]) -> Result<()> {
        let dir = self.path(module);
        let tmp = TempDir::new("pbs-devtools-gowork")?;
        let result = self.go_in(&dir, tmp.path(), args);
        let removed = tmp.remove();
        result?;
        removed
    }

    fn go_in(&self, dir: &Path, tmp: &Path, args: &[OsString]) -> Result<()> {
        let go_mod = dir.join("go.mod");
        let text = fs::read_to_string(&go_mod).map_err(|e| ToolError::io(&go_mod, &e))?;
        let version = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("go "))
            .map(str::trim)
            .ok_or_else(|| ToolError::new(format!("{} has no go directive", go_mod.display())))?;
        let work = tmp.join("go.work");
        let content = format!(
            "go {version}\n\nuse {}\n\nreplace {GO_MODULE} => {}\n",
            go_quote(dir)?,
            go_quote(&self.go_root)?
        );
        fs::write(&work, content).map_err(|e| ToolError::io(&work, &e))?;
        let sum = self.go_root.join("go.sum");
        let work_sum = tmp.join("go.work.sum");
        let _ = fs::copy(&sum, &work_sum).map_err(|e| ToolError::io(&sum, &e))?;

        let go = env::var_os("GO").unwrap_or_else(|| "go".into());
        let mut cmd = Command::new(&go);
        let _ = cmd.args(args).current_dir(dir).env("GOWORK", &work);
        for (key, _) in env::vars_os() {
            if key.to_string_lossy().starts_with("PBS_") {
                let _ = cmd.env_remove(key);
            }
        }
        let output = cmd.output().map_err(|e| {
            ToolError::new(format!(
                "running {}: {e} (is the Go toolchain installed? set GO to its path)",
                go.to_string_lossy()
            ))
        })?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = stderr.lines().collect();
        let tail = lines
            .get(lines.len().saturating_sub(40)..)
            .unwrap_or_default()
            .join("\n");
        let shown: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        Err(ToolError::new(format!(
            "go {} failed in {} ({}):\n{tail}",
            shown.join(" "),
            dir.display(),
            output.status
        )))
    }
}

fn join(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

/// `path` as a Go string literal, the form `go.work` takes paths in.
fn go_quote(path: &Path) -> Result<String> {
    let s = path
        .to_str()
        .ok_or_else(|| ToolError::new(format!("{} is not UTF-8", path.display())))?;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Ok(out)
}

/// Whether a generated file is written or compared against the committed copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Write the file.
    Write,
    /// Compare with the committed file and report staleness.
    Check,
}

/// The outcome of [`emit`] for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The file was written (it changed or did not exist).
    Written,
    /// The file already had this content.
    Unchanged,
    /// Check mode: the committed file differs or is missing.
    Stale,
}

/// Writes `content` to `path`, or in check mode compares it with the file on disk.
///
/// # Errors
///
/// Fails on I/O errors other than a missing file in check mode.
pub fn emit(path: &Path, content: &[u8], mode: Mode) -> Result<Outcome> {
    let current = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(ToolError::io(path, &e)),
    };
    if current.as_deref() == Some(content) {
        return Ok(Outcome::Unchanged);
    }
    if mode == Mode::Check {
        return Ok(Outcome::Stale);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ToolError::io(parent, &e))?;
    }
    fs::write(path, content).map_err(|e| ToolError::io(path, &e))?;
    Ok(Outcome::Written)
}

/// A fresh directory under the system temp dir; call [`TempDir::remove`] when done.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a new uniquely named directory.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be created.
    pub fn new(prefix: &str) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = env::temp_dir().join(format!("{prefix}-{}-{nanos}", process::id()));
        fs::create_dir_all(&path).map_err(|e| ToolError::io(&path, &e))?;
        Ok(Self { path })
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Deletes the directory and its contents.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be removed.
    pub fn remove(self) -> Result<()> {
        fs::remove_dir_all(&self.path).map_err(|e| ToolError::io(&self.path, &e))
    }
}

/// Serializes `value` as pretty JSON with a trailing newline.
///
/// # Errors
///
/// Fails when serialization fails.
pub fn to_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut out = serde_json::to_vec_pretty(value)?;
    out.push(b'\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{
        FETCH_SCRIPT, Mode, Outcome, PIN_FILE, Repo, TempDir, emit, go_quote, pinned_commit,
    };
    use crate::{
        error::Result,
        testutil::{check, check_eq},
    };

    #[test]
    fn locates_the_workspace_and_the_go_root() -> Result<()> {
        let repo = Repo::locate(None)?;
        check(
            repo.path("crates/devtools/Cargo.toml").is_file(),
            "this crate is in the workspace",
        )?;
        check(repo.path(FETCH_SCRIPT).is_file(), "the fetch script")?;
        check(repo.go_path("go.mod").is_file(), "go.mod in the Go root")?;
        check(
            repo.go_path("static/bidder-info").is_dir(),
            "static files in the Go root",
        )?;
        check(
            !repo.path("go.mod").exists() && !repo.path("static").exists(),
            "the workspace holds no upstream Go server files",
        )
    }

    /// The tests read the Go root as the goldens' commit left it: a clean checkout of the pin.
    #[test]
    fn go_root_is_a_clean_checkout_of_the_pinned_commit() -> Result<()> {
        let repo = Repo::locate(None)?;
        repo.verify_go_root(&repo.pinned_commit()?)
    }

    #[test]
    fn reads_the_pin() -> Result<()> {
        let dir = TempDir::new("pbs-devtools-pin")?;
        let file = dir.path().join(PIN_FILE);
        let sha = "4e296450de2fcc62741340b2e5f7d1df6c1744a5";
        fs::write(&file, format!("# comment\n\n{sha}\n"))?;
        check_eq(pinned_commit(dir.path())?.as_str(), sha, "one pin")?;
        fs::write(&file, format!("{sha}\n{sha}\n"))?;
        check(pinned_commit(dir.path()).is_err(), "two pins")?;
        fs::write(&file, "4e296450\n")?;
        check(pinned_commit(dir.path()).is_err(), "abbreviated")?;
        fs::write(&file, sha.to_uppercase())?;
        check(pinned_commit(dir.path()).is_err(), "upper case")?;
        dir.remove()
    }

    #[test]
    fn quotes_paths_for_go_work() -> Result<()> {
        check_eq(
            go_quote(Path::new(r#"/a b/"c"\d"#))?.as_str(),
            r#""/a b/\"c\"\\d""#,
            "quoted",
        )
    }

    #[test]
    fn emit_writes_then_reports_unchanged_and_stale() -> Result<()> {
        let dir = TempDir::new("pbs-devtools-test")?;
        let file = dir.path().join("sub").join("x.json");
        check_eq(
            &emit(&file, b"a", Mode::Check)?,
            &Outcome::Stale,
            "missing is stale",
        )?;
        check_eq(
            &emit(&file, b"a", Mode::Write)?,
            &Outcome::Written,
            "written",
        )?;
        check_eq(
            &emit(&file, b"a", Mode::Check)?,
            &Outcome::Unchanged,
            "same",
        )?;
        check_eq(
            &emit(&file, b"b", Mode::Check)?,
            &Outcome::Stale,
            "different",
        )?;
        check_eq(
            fs::read(&file)?.as_slice(),
            b"a".as_slice(),
            "check does not write",
        )?;
        dir.remove()
    }
}
