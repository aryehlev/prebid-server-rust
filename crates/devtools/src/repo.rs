//! Locating the repository, the Go commit the goldens derive from, and writing or checking
//! generated files.

use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::error::{Result, ToolError};

/// The Go module path of the repository root.
pub const GO_MODULE: &str = "github.com/prebid/prebid-server/v4";

/// Directory, relative to the root, that holds the generated goldens.
pub const TESTDATA: &str = "rust/testdata";

/// Paths whose history defines `go_commit`: the Go code and the data the goldens are read from.
/// The Rust workspace is excluded so regenerating the goldens never moves the commit.
const GO_PATHSPEC: &[&str] = &[
    "*.go",
    "go.mod",
    "go.sum",
    "static",
    "adapters",
    ":(exclude)rust",
];

/// The repository a command operates on.
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Uses `root` if given, then `PBS_GO_ROOT`, then the nearest ancestor of the current
    /// directory (and of this crate's source directory) whose `go.mod` declares [`GO_MODULE`].
    ///
    /// # Errors
    ///
    /// Fails when no such directory is found or the given one is not the repository root.
    pub fn locate(root: Option<&Path>) -> Result<Self> {
        if let Some(root) = root {
            return Self::at(root);
        }
        if let Some(from_env) = env::var_os("PBS_GO_ROOT") {
            return Self::at(Path::new(&from_env));
        }
        let cwd = env::current_dir()?;
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for start in [cwd.as_path(), manifest_dir.as_path()] {
            if let Some(found) = start.ancestors().find(|dir| is_root(dir)) {
                return Self::at(found);
            }
        }
        Err(ToolError::new(format!(
            "cannot find the prebid-server repository root (a go.mod declaring {GO_MODULE}); pass \
             --root or set PBS_GO_ROOT"
        )))
    }

    fn at(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root).map_err(|e| ToolError::io(root, &e))?;
        if !is_root(&root) {
            return Err(ToolError::new(format!(
                "{} is not the prebid-server repository root (no go.mod declaring {GO_MODULE})",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    /// The repository root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `rel` (slash-separated) joined to the root.
    #[must_use]
    pub fn path(&self, rel: &str) -> PathBuf {
        rel.split('/')
            .fold(self.root.clone(), |p, part| p.join(part))
    }

    /// The commit recorded as `go_commit`: `override_commit`, else `PBS_GO_COMMIT`, else the last
    /// commit touching `*.go`, `go.mod`, `go.sum`, `static/` or `adapters/` outside `rust/`.
    ///
    /// # Errors
    ///
    /// Fails when git is unavailable, the clone is shallow (its history cannot answer the
    /// question), or the history has no matching commit.
    pub fn go_commit(&self, override_commit: Option<&str>) -> Result<String> {
        if let Some(c) = override_commit {
            return Ok(c.to_owned());
        }
        if let Some(c) = env::var("PBS_GO_COMMIT").ok().filter(|c| !c.is_empty()) {
            return Ok(c);
        }
        let shallow = self.git(&["rev-parse", "--is-shallow-repository"])?;
        if shallow.trim() == "true" {
            return Err(ToolError::new(
                "the clone is shallow, so the last Go commit cannot be determined; fetch full \
                 history or set PBS_GO_COMMIT",
            ));
        }
        let mut args = vec!["log", "-1", "--format=%H", "--"];
        args.extend_from_slice(GO_PATHSPEC);
        let commit = self.git(&args)?.trim().to_owned();
        if commit.is_empty() {
            return Err(ToolError::new(
                "git log found no commit touching the Go sources",
            ));
        }
        Ok(commit)
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.root)
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
}

fn is_root(dir: &Path) -> bool {
    fs::read_to_string(dir.join("go.mod")).is_ok_and(|s| {
        s.lines()
            .next()
            .is_some_and(|l| l.trim() == format!("module {GO_MODULE}"))
    })
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
    use std::fs;

    use super::{Mode, Outcome, Repo, TempDir, emit};
    use crate::{
        error::Result,
        testutil::{check, check_eq},
    };

    #[test]
    fn locates_this_repository() -> Result<()> {
        let repo = Repo::locate(None)?;
        check(repo.path("go.mod").is_file(), "go.mod at root")?;
        check(
            repo.path("rust/crates/devtools/Cargo.toml").is_file(),
            "this crate",
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
