//! Checks of the Go tooling modules: the fork's Go code is test tooling, every root of it is its
//! own module, and each module builds against the upstream module at the pinned commit with the
//! upstream dependency versions, so the goldens record the code production runs.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

use crate::{
    error::{Result, ToolError},
    repo::{DEFAULT_GO_ROOT, GO_MODULE, GO_TOOLS, Repo},
    testutil::{check, check_eq},
};

/// The module path prefix of the tooling modules: the repository, then the module's directory.
const MODULE_PREFIX: &str = "github.com/aryehlev/prebid-server-rust/";

/// The directives of a `go.mod` file that these checks read.
#[derive(Debug, Default)]
struct GoMod {
    module: String,
    go: String,
    /// Module path to version.
    require: BTreeMap<String, String>,
    /// The left side of each replacement (`path` or `path version`) to its right side.
    replace: BTreeMap<String, String>,
    /// Every other directive, by keyword.
    other: Vec<String>,
}

impl GoMod {
    fn parse(text: &str) -> Result<Self> {
        let mut m = Self::default();
        let mut block: Option<String> = None;
        for raw in text.lines() {
            let line = raw.split("//").next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if let Some(kind) = &block {
                if line == ")" {
                    block = None;
                } else {
                    m.add(kind, line)?;
                }
                continue;
            }
            let (keyword, rest) = line.split_once(' ').unwrap_or((line, ""));
            let rest = rest.trim();
            match keyword {
                "module" => rest.clone_into(&mut m.module),
                "go" => rest.clone_into(&mut m.go),
                "require" | "replace" if rest == "(" => block = Some(keyword.to_owned()),
                "require" | "replace" => m.add(keyword, rest)?,
                other => m.other.push(other.to_owned()),
            }
        }
        Ok(m)
    }

    fn add(&mut self, kind: &str, line: &str) -> Result<()> {
        if kind == "replace" {
            let (left, right) = line
                .split_once("=>")
                .ok_or_else(|| ToolError::new(format!("replace without =>: {line}")))?;
            let _previous = self
                .replace
                .insert(left.trim().to_owned(), right.trim().to_owned());
            return Ok(());
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let [path, version] = words.as_slice() else {
            return Err(ToolError::new(format!("malformed require: {line}")));
        };
        let _previous = self
            .require
            .insert((*path).to_owned(), (*version).to_owned());
        Ok(())
    }
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| ToolError::io(path, &e))
}

/// Every directory below `dir` (workspace-relative, slash-separated) holding a file for which
/// `wanted` holds, skipping build output and hidden directories.
fn dirs_with(
    repo: &Repo,
    dir: &str,
    wanted: &dyn Fn(&str) -> bool,
    out: &mut BTreeSet<String>,
) -> Result<()> {
    let path = repo.path(dir);
    for entry in fs::read_dir(&path).map_err(|e| ToolError::io(&path, &e))? {
        let entry = entry.map_err(|e| ToolError::io(&path, &e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{dir}/{name}");
        if entry
            .file_type()
            .map_err(|e| ToolError::io(&entry.path(), &e))?
            .is_dir()
        {
            if name != "target" && !name.starts_with('.') {
                dirs_with(repo, &rel, wanted, out)?;
            }
        } else if wanted(&name) {
            let _ = out.insert(dir.to_owned());
        }
    }
    Ok(())
}

/// The pseudo-version suffix of `commit`: its UTC commit time and its first twelve hex digits.
fn pseudo_version_suffix(repo: &Repo, commit: &str) -> Result<String> {
    let out = Command::new("git")
        .args([
            "show",
            "-s",
            "--format=%cd",
            "--date=format-local:%Y%m%d%H%M%S",
            commit,
        ])
        .current_dir(repo.go_root())
        .env("TZ", "UTC")
        .output()
        .map_err(|e| ToolError::new(format!("running git: {e}")))?;
    if !out.status.success() {
        return Err(ToolError::new(format!(
            "git show {commit} in the Go root failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let stamp = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let short = commit
        .get(..12)
        .ok_or_else(|| ToolError::new(format!("{commit} is too short")))?;
    Ok(format!("{stamp}-{short}"))
}

/// Every `.go` file of the workspace is in a tooling module, and the tooling modules are exactly
/// `crates/devtools/go` and `crates/<crate>/testdata/go` directories.
#[test]
fn go_code_lives_in_tooling_modules() -> Result<()> {
    let repo = Repo::locate(None)?;
    let mut modules = BTreeSet::new();
    dirs_with(&repo, "crates", &|n| n == "go.mod", &mut modules)?;
    let mut go_dirs = BTreeSet::new();
    let is_go = |n: &str| Path::new(n).extension().is_some_and(|e| e == "go");
    dirs_with(&repo, "crates", &is_go, &mut go_dirs)?;
    check(modules.contains(GO_TOOLS), "the devtools Go module")?;
    let misplaced: Vec<&String> = modules
        .iter()
        .filter(|m| {
            m.as_str() != GO_TOOLS && !(m.ends_with("/testdata/go") && m.matches('/').count() == 3)
        })
        .collect();
    check_eq(
        misplaced.as_slice(),
        &[][..],
        "go.mod files outside crates/devtools/go and crates/*/testdata/go",
    )?;
    let orphans: Vec<&String> = go_dirs
        .iter()
        .filter(|d| {
            !modules
                .iter()
                .any(|m| d.as_str() == m || d.starts_with(&format!("{m}/")))
        })
        .collect();
    check_eq(
        orphans.as_slice(),
        &[][..],
        "Go code outside a tooling module",
    )
}

/// Each tooling module is named after its directory, has the upstream module's `go` version,
/// requires only modules the upstream module requires, at the upstream versions, and requires
/// the upstream module itself (when it imports it) at the pinned commit's pseudo-version,
/// replaced by `.upstream` at the workspace root.
#[test]
fn tooling_modules_follow_upstream() -> Result<()> {
    let repo = Repo::locate(None)?;
    let upstream = GoMod::parse(&read(&repo.go_path("go.mod"))?)?;
    check_eq(upstream.module.as_str(), GO_MODULE, "upstream module path")?;
    let suffix = pseudo_version_suffix(&repo, &repo.pinned_commit()?)?;
    let mut modules = BTreeSet::new();
    dirs_with(&repo, "crates", &|n| n == "go.mod", &mut modules)?;
    check(modules.len() > 10, "the tooling modules are found")?;
    let mut problems = Vec::new();
    for rel in &modules {
        let m = GoMod::parse(&read(&repo.path(&format!("{rel}/go.mod")))?)?;
        if m.module != format!("{MODULE_PREFIX}{rel}") {
            problems.push(format!("{rel}: module {}", m.module));
        }
        if m.go != upstream.go {
            problems.push(format!("{rel}: go {} (upstream: {})", m.go, upstream.go));
        }
        if !m.other.is_empty() {
            problems.push(format!("{rel}: unexpected directives {:?}", m.other));
        }
        for (path, version) in &m.require {
            if path == GO_MODULE {
                if !version.ends_with(&suffix) {
                    problems.push(format!(
                        "{rel}: requires {GO_MODULE} {version}, not the pinned commit's \
                         pseudo-version (ending {suffix})"
                    ));
                }
                continue;
            }
            match upstream.require.get(path) {
                Some(want) if want == version => {}
                Some(want) => {
                    problems.push(format!("{rel}: requires {path} {version}, upstream {want}"))
                }
                None => problems.push(format!(
                    "{rel}: requires {path}, which the upstream module does not"
                )),
            }
        }
        let depth = rel.matches('/').count() + 1;
        let target = format!("{}{DEFAULT_GO_ROOT}", "../".repeat(depth));
        let want: BTreeMap<String, String> = if m.require.contains_key(GO_MODULE) {
            BTreeMap::from([(GO_MODULE.to_owned(), target)])
        } else {
            BTreeMap::new()
        };
        if m.replace != want {
            problems.push(format!("{rel}: replace {:?}, want {want:?}", m.replace));
        }
    }
    check_eq(problems.as_slice(), &[][..], "tooling go.mod files")
}

#[test]
fn parses_go_mod() -> Result<()> {
    let m = GoMod::parse(
        "module example.com/m\n\ngo 1.25.0\n\nrequire a.com/x v1.0.0 // indirect\n\nrequire (\n\
         \tb.com/y v0.1.0\n\tc.com/z v2.0.0+incompatible // indirect\n)\n\n\
         replace a.com/x => ../x\n\ntoolchain go1.26.2\n",
    )?;
    check_eq(m.module.as_str(), "example.com/m", "module")?;
    check_eq(m.go.as_str(), "1.25.0", "go")?;
    check_eq(&m.require.len(), &3, "requires")?;
    check_eq(
        &m.require.get("c.com/z").map(String::as_str),
        &Some("v2.0.0+incompatible"),
        "block require",
    )?;
    check_eq(
        &m.replace.get("a.com/x").map(String::as_str),
        &Some("../x"),
        "replace",
    )?;
    check_eq(m.other.as_slice(), &["toolchain".to_owned()][..], "other")
}
