//! Pins: `crates/<crate>/<path>.rs::<test>`, a test function in a workspace file.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::pattern::is_identifier;

/// A pin, split into its file and its test function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pin {
    /// The file, relative to the workspace root, with `/` separators.
    file: String,
    /// The test function's name.
    test: String,
}

impl Pin {
    /// Parses `text`: a relative path under `crates/` to a `.rs` file, `::` and the name of a
    /// function.
    ///
    /// # Errors
    ///
    /// When `text` has another shape.
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let (file, function) = text
            .split_once("::")
            .ok_or_else(|| format!("pin {text:?} is not <file>::<test>"))?;
        if !is_identifier(function) {
            return Err(format!("pin {text:?}: {function:?} is not a function name"));
        }
        let parts: Vec<&str> = file.split('/').collect();
        let plain = parts
            .iter()
            .all(|part| !part.is_empty() && *part != "." && *part != ".." && !part.contains('\\'));
        if !plain
            || parts.first() != Some(&"crates")
            || parts.len() < 3
            || Path::new(file).extension() != Some(OsStr::new("rs"))
        {
            return Err(format!(
                "pin {text:?}: the file is not a relative path to a .rs file under crates/"
            ));
        }
        Ok(Self {
            file: file.to_owned(),
            test: function.to_owned(),
        })
    }
}

/// What a workspace has at a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Found {
    /// A test function of that name in that file.
    Test,
    /// The file exists, but has no test function of that name.
    NoTest,
    /// The file does not exist.
    NoFile,
}

/// The workspace's source files, read once each.
#[derive(Debug)]
pub(super) struct Sources {
    /// The workspace root.
    root: PathBuf,
    /// Each file read so far, `None` when it does not exist.
    files: BTreeMap<String, Option<String>>,
}

impl Sources {
    /// Sources under `root`.
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            files: BTreeMap::new(),
        }
    }

    /// Looks `pin` up.
    ///
    /// # Errors
    ///
    /// When the file exists but cannot be read.
    pub(super) fn find(&mut self, pin: &Pin) -> Result<Found, String> {
        if !self.files.contains_key(&pin.file) {
            let path = self.root.join(&pin.file);
            let text = match fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            let _previous = self.files.insert(pin.file.clone(), text);
        }
        Ok(match self.files.get(&pin.file) {
            Some(Some(text)) if has_test(text, &pin.test) => Found::Test,
            Some(Some(_)) => Found::NoTest,
            Some(None) | None => Found::NoFile,
        })
    }
}

/// Whether `source` defines a test function named `name`: a `fn name(` line (`pub` and `async`
/// allowed) whose attributes above it hold `#[test]` or `#[tokio::test`.
fn has_test(source: &str, name: &str) -> bool {
    let lines: Vec<&str> = source.lines().collect();
    lines.iter().enumerate().any(|(at, line)| {
        defines(line.trim_start(), name) && lines.get(..at).is_some_and(test_attribute)
    })
}

/// Whether `line` starts the definition of a function named `name`.
fn defines(line: &str, name: &str) -> bool {
    let line = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub(super) "))
        .or_else(|| line.strip_prefix("pub "))
        .unwrap_or(line);
    let line = line.strip_prefix("async ").unwrap_or(line);
    line.strip_prefix("fn ")
        .and_then(|rest| rest.strip_prefix(name))
        .is_some_and(|rest| rest.starts_with('(') || rest.starts_with('<'))
}

/// Whether the attribute and comment lines right above a function (the end of `above`) hold a
/// test attribute. The walk stops at a blank line or at the end of the item before.
fn test_attribute(above: &[&str]) -> bool {
    for line in above.iter().rev().map(|line| line.trim()) {
        if line.starts_with("#[test]") || line.starts_with("#[tokio::test") {
            return true;
        }
        if line.starts_with("//") {
            continue;
        }
        if line.is_empty() || line.ends_with('}') || line.ends_with('{') || line.ends_with(';') {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::{env, fs, process};

    use super::{Found, Pin, Sources, has_test};
    use crate::{TestResult, check, check_eq};

    #[test]
    fn pins_parse() -> TestResult {
        let pin = Pin::parse("crates/differences/tests/json.rs::go_difference_invalid_utf8")?;
        check_eq(
            pin.file.as_str(),
            "crates/differences/tests/json.rs",
            "file",
        )?;
        check_eq(pin.test.as_str(), "go_difference_invalid_utf8", "test")?;
        for text in [
            "crates/differences/tests/json.rs",
            "crates/differences/tests/json.rs::",
            "crates/differences/tests/json.rs::not-a-name",
            "tests/json.rs::x",
            "/crates/differences/tests/json.rs::x",
            "crates/../tests/json.rs::x",
            "crates/differences//json.rs::x",
            "crates/differences/tests/json.go::x",
            "crates/json.rs::x",
            "crates\\differences\\json.rs::x",
        ] {
            check(Pin::parse(text).is_err(), text)?;
        }
        Ok(())
    }

    #[test]
    fn test_functions_are_found() -> TestResult {
        let source = "\
#[cfg(test)]
mod tests {
    use super::x;

    /// Doc.
    // Go: somewhere.go:1
    #[test]
    fn plain() -> TestResult {
        Ok(())
    }

    #[tokio::test(
        flavor = \"multi_thread\",
        worker_threads = 4
    )]
    async fn wrapped() {}

    fn helper() {}

    #[test]

    fn after_blank() {}

    #[test]
    fn generic<T>() {}

    #[test]
    pub(crate) fn crate_visible() {}

    #[test]
    pub(super) async fn super_visible() {}

    /// Names `plain` in a comment: fn plain() is not a definition here.
    fn not_a_test() {}
}
";
        check(has_test(source, "plain"), "#[test]")?;
        check(has_test(source, "wrapped"), "a wrapped #[tokio::test(...)]")?;
        check(has_test(source, "generic"), "generics")?;
        check(has_test(source, "crate_visible"), "pub(crate)")?;
        check(has_test(source, "super_visible"), "pub(super) async")?;
        check(!has_test(source, "helper"), "no attribute")?;
        check(!has_test(source, "after_blank"), "a blank line between")?;
        check(!has_test(source, "not_a_test"), "a comment is no attribute")?;
        check(!has_test(source, "plai"), "a prefix of a name")?;
        check(!has_test(source, "missing"), "no such function")
    }

    #[test]
    fn sources_report_files_and_tests() -> TestResult {
        let root = env::temp_dir().join(format!("pbs-differences-pins-{}", process::id()));
        let dir = root.join("crates/x/tests");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("a.rs"), "#[test]\nfn here() {}\n")?;
        let mut sources = Sources::new(&root);
        let found = [
            sources.find(&Pin::parse("crates/x/tests/a.rs::here")?)?,
            sources.find(&Pin::parse("crates/x/tests/a.rs::gone")?)?,
            sources.find(&Pin::parse("crates/x/tests/b.rs::here")?)?,
        ];
        fs::remove_dir_all(&root)?;
        check_eq(
            found.as_slice(),
            [Found::Test, Found::NoTest, Found::NoFile].as_slice(),
            "a test, a missing test, a missing file",
        )
    }
}
