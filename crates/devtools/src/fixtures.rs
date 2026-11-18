//! Which JSON fixtures `adapterstest.RunJSONBidderTest` runs for a root directory.
//!
//! Mirrors `adapters/adapterstest/test_json.go`: `filepath.WalkDir(rootDir)` visits every entry
//! (directories are not followed through symlinks), and a non-directory whose name ends in
//! `.json` runs when the base name of its parent directory is one of [`DIR_KINDS`]. Deeper
//! nesting under a kind directory is skipped, so `adprimetest/params/race/*.json` never runs.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::error::{Result, ToolError};

/// The directory names whose JSON files run (`supportedDirs` in `test_json.go`).
pub const DIR_KINDS: [&str; 5] = [
    "amp",
    "exemplary",
    "supplemental",
    "video",
    "videosupplemental",
];

/// One fixture file a run executes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fixture {
    /// The parent directory name, one of [`DIR_KINDS`].
    pub kind: &'static str,
    /// The file path.
    pub path: PathBuf,
}

/// Every fixture under `root` in `filepath.WalkDir` (lexical) order.
///
/// # Errors
///
/// Fails when `root` or a directory below it cannot be read, as Go's walk would.
pub fn walk(root: &Path) -> Result<Vec<Fixture>> {
    let mut out = Vec::new();
    visit(root, &mut out)?;
    Ok(out)
}

fn visit(dir: &Path, out: &mut Vec<Fixture>) -> Result<()> {
    let mut entries: Vec<(String, PathBuf, bool)> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| ToolError::io(dir, &e))? {
        let entry = entry.map_err(|e| ToolError::io(dir, &e))?;
        let name = entry.file_name().into_string().map_err(|n| {
            ToolError::new(format!(
                "{}: non-UTF-8 file name {}",
                dir.display(),
                n.display()
            ))
        })?;
        let is_dir = entry
            .file_type()
            .map_err(|e| ToolError::io(&entry.path(), &e))?
            .is_dir();
        entries.push((name, entry.path(), is_dir));
    }
    // os.ReadDir sorts by file name.
    entries.sort();
    let parent = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let kind = DIR_KINDS.iter().find(|k| **k == parent);
    for (name, path, is_dir) in entries {
        if is_dir {
            visit(&path, out)?;
        } else if let Some(kind) = kind
            && go_ext(&name) == ".json"
        {
            out.push(Fixture { kind, path });
        }
    }
    Ok(())
}

/// `filepath.Ext`: the suffix starting at the final dot of the last element, or "".
fn go_ext(name: &str) -> &str {
    name.rfind('.')
        .and_then(|i| name.get(i..))
        .unwrap_or_default()
}

/// Counts fixtures per kind, with every kind present (zero when absent).
#[must_use]
pub fn count(fixtures: &[Fixture]) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = DIR_KINDS.iter().map(|k| ((*k).to_owned(), 0)).collect();
    for f in fixtures {
        *out.entry(f.kind.to_owned()).or_insert(0) += 1;
    }
    out
}

/// Every `.json` file below `dir`, sorted.
///
/// # Errors
///
/// Fails when a directory cannot be read.
pub fn all_json(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).map_err(|e| ToolError::io(&d, &e))? {
            let entry = entry.map_err(|e| ToolError::io(&d, &e))?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|e| ToolError::io(&path, &e))?
                .is_dir()
            {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{count, go_ext, walk};
    use crate::{error::Result, repo::TempDir, testutil::check_eq};

    #[test]
    fn go_ext_matches_filepath_ext() -> Result<()> {
        check_eq(go_ext("a.json"), ".json", "plain")?;
        check_eq(go_ext("a.JSON"), ".JSON", "case kept")?;
        check_eq(go_ext(".json"), ".json", "dot file")?;
        check_eq(go_ext("a.json.bak"), ".bak", "last dot")?;
        check_eq(go_ext("json"), "", "no dot")
    }

    #[test]
    fn walk_follows_test_json_rules() -> Result<()> {
        let dir = TempDir::new("pbs-devtools-fixtures")?;
        let root = dir.path().join("xtest");
        for rel in [
            "exemplary/a.json",
            "exemplary/b.JSON",
            "exemplary/nested/c.json",
            "supplemental/d.json",
            "params/race/e.json",
            "other/exemplary/f.json",
            "video/g.txt",
            "videosupplemental/h.json",
        ] {
            let path = rel.split('/').fold(root.clone(), |p, s| p.join(s));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, "{}")?;
        }
        fs::create_dir_all(root.join("amp").join("dir.json"))?;
        let found = walk(&root)?;
        let names: Vec<String> = found
            .iter()
            .filter_map(|f| f.path.strip_prefix(&root).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        check_eq(
            names.as_slice(),
            &[
                "exemplary/a.json",
                "other/exemplary/f.json",
                "supplemental/d.json",
                "videosupplemental/h.json",
            ]
            .map(str::to_owned)[..],
            "walk order and filtering",
        )?;
        let counts = count(&found);
        check_eq(&counts.get("exemplary"), &Some(&2), "exemplary")?;
        check_eq(&counts.get("amp"), &Some(&0), "amp present with zero")?;
        dir.remove()
    }
}
