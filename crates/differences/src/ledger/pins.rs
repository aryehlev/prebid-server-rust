//! Pins: `crates/<crate>/tests/<path>.rs::<test>` or `crates/<crate>/src/<path>.rs::<test>`, a
//! test function `cargo test` runs. syn parses the file, so a function in a comment, a doc comment
//! or a string is no test, and one marked `#[ignore]` pins nothing.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use syn::{Attribute, Item, parse_file};

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
    /// Parses `text`: a relative path to a `.rs` file under a crate's `tests/` or `src/`, where
    /// `cargo test` finds tests, `::` and the name of a function.
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
            || parts.len() < 4
            || !matches!(parts.get(2).copied(), Some("tests" | "src"))
            || Path::new(file).extension() != Some(OsStr::new("rs"))
        {
            return Err(format!(
                "pin {text:?}: the file is not a relative path to a .rs file under \
                 crates/<crate>/tests/ or crates/<crate>/src/, where cargo test finds tests"
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
    /// A test function of that name in that file, which `cargo test` runs.
    Test,
    /// Test functions of that name, each marked `#[ignore]`, so `cargo test` runs none.
    Ignored,
    /// The file exists, but has no test function of that name.
    NoTest,
    /// The file does not exist.
    NoFile,
}

/// The test functions of a file by name, each with whether every one of that name is ignored.
type Tests = BTreeMap<String, bool>;

/// The workspace's source files, parsed once each.
#[derive(Debug)]
pub(super) struct Sources {
    /// The workspace root.
    root: PathBuf,
    /// The test functions of each file parsed so far, `None` when the file does not exist.
    files: BTreeMap<String, Option<Tests>>,
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
    /// When the file exists but cannot be read, or syn cannot parse it.
    pub(super) fn find(&mut self, pin: &Pin) -> Result<Found, String> {
        if !self.files.contains_key(&pin.file) {
            let path = self.root.join(&pin.file);
            let parsed = match fs::read_to_string(&path) {
                Ok(text) => {
                    Some(test_functions(&text).map_err(|e| format!("{}: {e}", path.display()))?)
                }
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            let _previous = self.files.insert(pin.file.clone(), parsed);
        }
        Ok(match self.files.get(&pin.file) {
            Some(Some(tests)) => match tests.get(&pin.test) {
                Some(false) => Found::Test,
                Some(true) => Found::Ignored,
                None => Found::NoTest,
            },
            Some(None) | None => Found::NoFile,
        })
    }
}

/// The test functions `source` defines, at its top level and in its inline modules: functions
/// with a `#[test]` or `#[tokio::test]` attribute, which `cargo test` runs unless `#[ignore]`
/// marks them.
///
/// # Errors
///
/// When syn cannot parse `source` as a Rust file.
fn test_functions(source: &str) -> Result<Tests, String> {
    let file = parse_file(source).map_err(|e| format!("syn cannot parse the file: {e}"))?;
    let mut found = Tests::new();
    collect(&file.items, &mut found);
    Ok(found)
}

/// Adds the test functions among `items`, and in the inline modules among them, to `found`.
fn collect(items: &[Item], found: &mut Tests) {
    for item in items {
        if let Item::Fn(function) = item
            && function.attrs.iter().any(is_test)
        {
            let ignored = function
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("ignore"));
            let name = function.sig.ident.to_string();
            let every_one_ignored = found
                .get(&name)
                .map_or(ignored, |earlier| *earlier && ignored);
            let _previous = found.insert(name, every_one_ignored);
        } else if let Item::Mod(module) = item
            && let Some((_, inner)) = &module.content
        {
            collect(inner, found);
        }
    }
}

/// Whether `attr` is `#[test]` or `#[tokio::test]`, with or without arguments.
fn is_test(attr: &Attribute) -> bool {
    let path: Vec<String> = attr
        .path()
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    path == ["test"] || path == ["tokio", "test"]
}

#[cfg(test)]
mod tests {
    use std::{env, fs, process};

    use super::{Found, Pin, Sources, test_functions};
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
        let unit = Pin::parse("crates/differences/src/ledger/pins.rs::pins_parse")?;
        check_eq(
            unit.file.as_str(),
            "crates/differences/src/ledger/pins.rs",
            "a unit test under src/",
        )?;
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
            "crates/differences/build.rs::x",
            "crates/differences/testdata/go/pins.rs::x",
            "crates/differences/benches/json.rs::x",
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
    pub(crate) fn crate_visible() {}

    #[test]
    pub(super) async fn super_visible() {}

    #[test]
    #[ignore = \"slow\"]
    fn ignored() {}

    mod nested {
        #[test]
        fn inner() {}
    }

    mod first {
        #[test]
        #[ignore]
        fn twice() {}
    }

    mod second {
        #[test]
        fn twice() {}
    }

    /*
    #[test]
    fn commented_out() {}
    */

    /// Names `plain` in a comment: fn plain() is not a definition here.
    /// #[test]
    /// fn in_a_doc() {}
    fn not_a_test() {}

    const TEXT: &str = \"#[test] fn in_a_string() {}\";
}
";
        let tests = test_functions(source)?;
        let found = |name: &str| tests.get(name).copied();
        check_eq(&found("plain"), &Some(false), "#[test]")?;
        check_eq(
            &found("wrapped"),
            &Some(false),
            "a wrapped #[tokio::test(...)]",
        )?;
        check_eq(&found("after_blank"), &Some(false), "a blank line between")?;
        check_eq(&found("crate_visible"), &Some(false), "pub(crate)")?;
        check_eq(&found("super_visible"), &Some(false), "pub(super) async")?;
        check_eq(&found("inner"), &Some(false), "a nested module")?;
        check_eq(&found("twice"), &Some(false), "one of two runs")?;
        check_eq(&found("ignored"), &Some(true), "#[ignore]")?;
        check_eq(&found("helper"), &None, "no attribute")?;
        check_eq(&found("commented_out"), &None, "a block comment")?;
        check_eq(&found("in_a_doc"), &None, "a doc comment")?;
        check_eq(&found("in_a_string"), &None, "a string")?;
        check_eq(&found("not_a_test"), &None, "a comment is no attribute")?;
        check_eq(&found("plai"), &None, "a prefix of a name")?;
        check_eq(&found("missing"), &None, "no such function")?;
        check(test_functions("fn broken( {").is_err(), "not Rust")
    }

    #[test]
    fn sources_report_files_and_tests() -> TestResult {
        let root = env::temp_dir().join(format!("pbs-differences-pins-{}", process::id()));
        let dir = root.join("crates/x/tests");
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("a.rs"),
            "#[test]\nfn here() {}\n\n#[test]\n#[ignore]\nfn skipped() {}\n",
        )?;
        fs::write(dir.join("broken.rs"), "fn here( {\n")?;
        let mut sources = Sources::new(&root);
        let found = [
            sources.find(&Pin::parse("crates/x/tests/a.rs::here")?)?,
            sources.find(&Pin::parse("crates/x/tests/a.rs::skipped")?)?,
            sources.find(&Pin::parse("crates/x/tests/a.rs::gone")?)?,
            sources.find(&Pin::parse("crates/x/tests/b.rs::here")?)?,
        ];
        let broken = sources.find(&Pin::parse("crates/x/tests/broken.rs::here")?);
        fs::remove_dir_all(&root)?;
        check_eq(
            found.as_slice(),
            [Found::Test, Found::Ignored, Found::NoTest, Found::NoFile].as_slice(),
            "a test, an ignored test, a missing test, a missing file",
        )?;
        check(
            broken.is_err_and(|e| e.contains("broken.rs: syn cannot parse the file")),
            "a file syn cannot parse",
        )
    }
}
