//! The generator's own tests: fragments in a scratch workspace, validated, written and checked.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    Command, FileState, JSON, LedgerError, MARKDOWN, Pending, USAGE, check as check_docs,
    parse_args, run, write as write_docs,
};
use crate::{BoxError, TestResult, check, check_eq};

/// The test functions the scratch fragments pin.
const PINS: &str = "\
#[cfg(test)]
mod tests {
    #[test]
    fn go_difference_one() {}

    #[test]
    fn go_difference_two() {}

    #[tokio::test]
    async fn restore_one() {}

    fn helper() {}

    #[test]
    #[ignore]
    fn go_difference_skipped() {}

    /*
    #[test]
    fn go_difference_commented() {}
    */
}
";

/// A valid ledgered entry.
const ONE: &str = r#"
[[difference]]
id = "json.one"
category = "json"
status = "ledgered"
title = "Go writes `<` escaped, serde_json as it is"
go = "Go escapes it."
rust = "serde_json does not."
who = ["publisher", "bidder"]
visible_in = "Response bodies"
pin = "crates/x/tests/pins.rs::go_difference_one"
restore = "A Formatter; not planned."

[[difference.harness]]
kind = "lexical"
lexical_class = "html_escape"
path = "$.**"
"#;

/// A valid restored entry whose restore pin is still to land.
const RESTORED: &str = r#"
[[difference]]
id = "config.two"
category = "config"
status = "restored"
title = "Keys"
go = "Viper lower-cases keys."
rust = "config keeps them."
who = ["operator"]
visible_in = "Startup"
pin = "crates/x/tests/pins.rs::go_difference_two"
rule_b = "c"
restore = "The loader lower-cases keys."
restore_by = ["CONFIG-LOADER"]
"#;

/// The restore pin of [`RESTORED`], as the restoring crate's fragment gives it.
const RESTORATION: &str = r#"
[[restoration]]
id = "config.two"
pin = "crates/x/tests/pins.rs::restore_one"
"#;

/// A scratch workspace with one crate, `x`, whose `tests/pins.rs` holds [`PINS`].
struct Scratch {
    /// The workspace root.
    root: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self, BoxError> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = env::temp_dir().join(format!(
            "pbs-differences-ledger-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        fs::create_dir_all(root.join("crates/x/tests"))?;
        fs::write(root.join("crates/x/tests/pins.rs"), PINS)?;
        Ok(Self { root })
    }

    fn root(&self) -> &Path {
        &self.root
    }

    /// Writes `crates/<krate>/differences.toml`.
    fn fragment(&self, krate: &str, text: &str) -> TestResult {
        let dir = self.root.join("crates").join(krate);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("differences.toml"), text)?;
        Ok(())
    }

    fn read(&self, path: &str) -> Result<String, BoxError> {
        Ok(fs::read_to_string(self.root.join(path))?)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap_or_default();
    }
}

/// `text` with its first `from` replaced by `to`; fails when `text` has no `from`.
fn edit(text: &str, from: &str, to: &str) -> Result<String, BoxError> {
    if text.contains(from) {
        Ok(text.replacen(from, to, 1))
    } else {
        Err(format!("the fragment has no {from:?}").into())
    }
}

/// Fails unless `result` is an error with a problem that contains `want`.
fn fails_with<T>(result: Result<T, LedgerError>, want: &str) -> TestResult {
    match result {
        Ok(_) => Err(format!("passed; want a problem with {want:?}").into()),
        Err(error) => check(
            error
                .problems()
                .iter()
                .any(|problem| problem.contains(want)),
            &format!("want a problem with {want:?}, got:\n{error}"),
        ),
    }
}

/// Validates the scratch workspace's ledger by writing it.
fn validate(scratch: &Scratch) -> Result<super::Summary, LedgerError> {
    write_docs(scratch.root(), Pending::Allowed)
}

#[test]
fn writes_then_checks() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment("x", ONE)?;
    let written = write_docs(scratch.root(), Pending::Allowed)?;
    check_eq(
        written.files(),
        [(MARKDOWN, FileState::Written), (JSON, FileState::Written)].as_slice(),
        "the first write",
    )?;
    let checked = check_docs(scratch.root(), Pending::Allowed)?;
    check_eq(
        checked.files(),
        [(MARKDOWN, FileState::Current), (JSON, FileState::Current)].as_slice(),
        "a check after the write",
    )?;
    let markdown = scratch.read(MARKDOWN)?;
    let again = write_docs(scratch.root(), Pending::Allowed)?;
    check_eq(
        again.files(),
        [(MARKDOWN, FileState::Current), (JSON, FileState::Current)].as_slice(),
        "a second write changes nothing",
    )?;
    check_eq(&scratch.read(MARKDOWN)?, &markdown, "the same bytes")?;
    check(
        markdown.contains("#### `json.one`: Go writes `<` escaped, serde_json as it is"),
        "the entry's heading",
    )?;
    check(
        markdown.contains("- **Harness:** lexical `html_escape` at `$.**`"),
        "the entry's pattern",
    )?;
    let json: Value = serde_json::from_str(&scratch.read(JSON)?)?;
    check_eq(
        &json.pointer("/schema").and_then(Value::as_str),
        &Some("pbs-differences/1"),
        "schema",
    )?;
    check_eq(
        &json
            .pointer("/differences/0/harness/0/lexical_class")
            .and_then(Value::as_str),
        &Some("html_escape"),
        "the pattern for the harness",
    )?;
    check_eq(
        &json.pointer("/fragments/0").and_then(Value::as_str),
        &Some("crates/x/differences.toml"),
        "the fragment read",
    )
}

#[test]
fn stale_or_missing_documents_fail_the_check() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment("x", ONE)?;
    let _written = write_docs(scratch.root(), Pending::Allowed)?;
    scratch.fragment("x", &edit(ONE, "Response bodies", "Bidder requests")?)?;
    let stale = check_docs(scratch.root(), Pending::Allowed);
    fails_with(
        stale.clone(),
        "docs/DIFFERENCES.md is stale: run cargo run -p pbs-differences",
    )?;
    fails_with(stale, "docs/differences.json is stale")?;
    let _rewritten = write_docs(scratch.root(), Pending::Allowed)?;
    fs::remove_file(scratch.root().join(JSON))?;
    fails_with(
        check_docs(scratch.root(), Pending::Allowed),
        "docs/differences.json is missing",
    )
}

#[test]
fn a_missing_pin_fails() -> TestResult {
    let scratch = Scratch::new()?;
    for (pin, want) in [
        (
            "crates/x/tests/pins.rs::go_difference_three",
            "the file has no test function of that name",
        ),
        (
            "crates/x/tests/pins.rs::helper",
            "the file has no test function of that name",
        ),
        (
            "crates/x/tests/pins.rs::go_difference_commented",
            "the file has no test function of that name",
        ),
        (
            "crates/x/tests/pins.rs::go_difference_skipped",
            "the test is marked #[ignore], so cargo test never runs it",
        ),
        ("crates/x/tests/gone.rs::go_difference_one", "no such file"),
        ("crates/x/tests/pins.rs", "is not <file>::<test>"),
        ("../x/tests/pins.rs::go_difference_one", "under crates/"),
        (
            "crates/x/testdata/pins.rs::go_difference_one",
            "where cargo test finds tests",
        ),
    ] {
        scratch.fragment(
            "x",
            &edit(ONE, "crates/x/tests/pins.rs::go_difference_one", pin)?,
        )?;
        fails_with(validate(&scratch), want)?;
    }
    Ok(())
}

#[test]
fn a_duplicate_id_fails() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment("x", ONE)?;
    scratch.fragment("y", ONE)?;
    fails_with(
        validate(&scratch),
        "crates/y/differences.toml: json.one: the id is already used in crates/x/differences.toml",
    )
}

#[test]
fn an_unknown_category_fails() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment(
        "x",
        &edit(ONE, "category = \"json\"", "category = \"jsn\"")?,
    )?;
    fails_with(
        validate(&scratch),
        "unknown category \"jsn\"; the categories are",
    )?;
    scratch.fragment(
        "x",
        &edit(ONE, "category = \"json\"", "category = \"yaml\"")?,
    )?;
    fails_with(
        validate(&scratch),
        "the id does not start with its category, yaml.",
    )
}

#[test]
fn a_wildcard_path_on_a_value_kind_fails() -> TestResult {
    let scratch = Scratch::new()?;
    for kind in ["value", "type", "missing_in_rust", "extra_in_rust"] {
        for path in ["$.**", "$.seatbid[*].*"] {
            let fragment = edit(ONE, "kind = \"lexical\"", &format!("kind = \"{kind}\""))?
                .replace("lexical_class = \"html_escape\"\n", "")
                .replace("path = \"$.**\"", &format!("path = \"{path}\""));
            scratch.fragment("x", &fragment)?;
            fails_with(
                validate(&scratch),
                &format!("a {kind} pattern names a concrete path, without .* or .**, not {path}"),
            )?;
        }
    }
    let concrete = edit(ONE, "kind = \"lexical\"", "kind = \"value\"")?
        .replace("lexical_class = \"html_escape\"\n", "")
        .replace("path = \"$.**\"", "path = \"$.seatbid[*].bid[*].price\"");
    scratch.fragment("x", &concrete)?;
    let _summary = validate(&scratch)?;
    Ok(())
}

#[test]
fn harness_patterns_obey_the_rules() -> TestResult {
    let scratch = Scratch::new()?;
    let pattern = |kind: &str, class: Option<&str>, path: &str, input: Option<&str>| {
        let class = class.map_or_default(|c| format!("lexical_class = \"{c}\"\n"));
        let input = input.map_or_default(|i| format!("input = \"{i}\"\n"));
        format!("[[difference.harness]]\nkind = \"{kind}\"\n{class}path = \"{path}\"\n{input}")
    };
    let head = ONE
        .split("[[difference.harness]]")
        .next()
        .ok_or("the entry")?
        .to_owned();
    for (with, want) in [
        (
            pattern("status", None, "$['@status']", None),
            "a status pattern names an input",
        ),
        (
            pattern("status", None, "$['@status']", Some("dup")),
            "unknown input predicate \"dup\"",
        ),
        (
            pattern("lexical", None, "$.**", None),
            "kind lexical names its lexical_class",
        ),
        (
            pattern("lexical", Some("other"), "$.**", None),
            "no entry names class other",
        ),
        (
            pattern("lexical", Some("header_case"), "$.**", None),
            "kind lexical has no class \"header_case\"",
        ),
        (
            pattern("value", Some("html_escape"), "$.id", None),
            "kind value takes no lexical_class",
        ),
        (
            pattern("stage_missing", None, "$", None),
            "no entry names kind stage_missing",
        ),
        (
            pattern("values", None, "$", None),
            "unknown kind \"values\"",
        ),
        (pattern("header", None, "$['id']", None), "write it $.id"),
        (
            pattern("header", None, "$.imp[0]", None),
            "an index is written [*]",
        ),
    ] {
        scratch.fragment("x", &format!("{head}{with}"))?;
        fails_with(validate(&scratch), want)?;
    }
    let fine = pattern("status", None, "$['@status']", Some("duplicate_keys"))
        + &pattern(
            "wire",
            Some("default_header"),
            "$[*].request.headers.Accept",
            None,
        );
    scratch.fragment("x", &format!("{head}{fine}"))?;
    let _summary = validate(&scratch)?;
    Ok(())
}

#[test]
fn overlapping_patterns_fail() -> TestResult {
    let scratch = Scratch::new()?;
    let two = edit(ONE, "id = \"json.one\"", "id = \"json.two\"")?
        .replace("path = \"$.**\"", "path = \"$.ext.price\"");
    scratch.fragment("x", &format!("{ONE}{two}"))?;
    fails_with(
        validate(&scratch),
        "json.one: harness pattern 1 (lexical html_escape at $.**) overlaps pattern 1 of \
         json.two (lexical html_escape at $.ext.price)",
    )?;
    let status = |id: &str, input: &str| {
        format!(
            "{}[[difference.harness]]\nkind = \"status\"\npath = \"$['@status']\"\ninput = \
             \"{input}\"\n",
            ONE.split("[[difference.harness]]")
                .next()
                .unwrap_or_default()
                .replace("json.one", id)
        )
    };
    scratch.fragment(
        "x",
        &(status("json.a", "duplicate_keys") + &status("json.b", "invalid_utf8")),
    )?;
    let _summary = validate(&scratch)?;
    scratch.fragment(
        "x",
        &(status("json.a", "duplicate_keys") + &status("json.b", "duplicate_keys")),
    )?;
    fails_with(validate(&scratch), "one mismatch would match both")
}

#[test]
fn restored_entries_name_why_and_their_restore_pin() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment("x", RESTORED)?;
    let summary = validate(&scratch)?;
    check_eq(
        summary.pending(),
        ["config.two (CONFIG-LOADER)".to_owned()].as_slice(),
        "pending until the restore pin lands",
    )?;
    check(
        scratch
            .read(MARKDOWN)?
            .contains("- **Restore pin:** pending, brought by CONFIG-LOADER"),
        "the document says so",
    )?;
    fails_with(
        write_docs(scratch.root(), Pending::Refused),
        "restore pin pending: config.two (CONFIG-LOADER)",
    )?;
    scratch.fragment("y", RESTORATION)?;
    let landed = write_docs(scratch.root(), Pending::Refused)?;
    check(landed.pending().is_empty(), "the restoration landed")?;
    check(
        scratch
            .read(MARKDOWN)?
            .contains("- **Restore pin:** `crates/x/tests/pins.rs::restore_one`"),
        "the restore pin in the document",
    )?;
    for (from, to, want) in [
        (
            "rule_b = \"c\"\n",
            "",
            "names its Rule B case, or prebid-logic",
        ),
        (
            "rule_b = \"c\"",
            "rule_b = \"e\"",
            "rule_b \"e\" is none of",
        ),
        (
            "restore = \"The loader lower-cases keys.\"\n",
            "",
            "says what keeps Go's behaviour",
        ),
        (
            "restore_by = [\"CONFIG-LOADER\"]",
            "restore_by = [\"config-loader\"]",
            "is not a plan unit",
        ),
    ] {
        scratch.fragment("x", &edit(RESTORED, from, to)?)?;
        fails_with(validate(&scratch), want)?;
    }
    fs::remove_file(scratch.root().join("crates/y/differences.toml"))?;
    scratch.fragment(
        "x",
        &edit(RESTORED, "restore_by = [\"CONFIG-LOADER\"]\n", "")?,
    )?;
    fails_with(validate(&scratch), "a restored entry needs a restore pin")?;
    let decided = edit(RESTORED, "rule_b = \"c\"", "decision = \"D-UA\"")?;
    scratch.fragment("x", &decided)?;
    let _summary = validate(&scratch)?;
    let with_pattern = format!(
        "{RESTORED}[[difference.harness]]\nkind = \"header\"\npath = \"$['@headers'].Vary\"\n"
    );
    scratch.fragment("x", &with_pattern)?;
    fails_with(
        validate(&scratch),
        "a restored entry has no harness pattern",
    )
}

#[test]
fn restorations_name_restored_entries() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment("x", ONE)?;
    scratch.fragment("y", &edit(RESTORATION, "config.two", "json.one")?)?;
    fails_with(
        validate(&scratch),
        "crates/y/differences.toml: restoration of json.one: the entry is ledgered, not restored",
    )?;
    scratch.fragment("y", RESTORATION)?;
    fails_with(
        validate(&scratch),
        "restoration of config.two: no entry has that id",
    )?;
    scratch.fragment("x", RESTORED)?;
    scratch.fragment("y", &edit(RESTORATION, "restore_one", "missing")?)?;
    fails_with(
        validate(&scratch),
        "the file has no test function of that name",
    )?;
    scratch.fragment("y", &format!("{RESTORATION}{RESTORATION}"))?;
    fails_with(validate(&scratch), "already has restore pin")
}

#[test]
fn decision_pending_entries_give_the_default() -> TestResult {
    let scratch = Scratch::new()?;
    let pending = edit(
        ONE,
        "status = \"ledgered\"",
        "status = \"decision-pending\"",
    )?;
    scratch.fragment("x", &pending)?;
    fails_with(
        validate(&scratch),
        "a decision-pending entry names its decision",
    )?;
    fails_with(
        validate(&scratch),
        "says what applies until the user decides",
    )?;
    let decided = edit(
        &pending,
        "restore = \"A Formatter; not planned.\"",
        "restore = \"A Formatter.\"\ndecision = \"D-X\"\ndefault = \"serde_json's spelling.\"",
    )?;
    scratch.fragment("x", &decided)?;
    let _summary = validate(&scratch)?;
    check(
        scratch.read(MARKDOWN)?.contains(
            "(D-X): Go writes `<` escaped, serde_json as it is. Default until decided: \
                       serde_json's spelling.",
        ),
        "the decisions section",
    )?;
    check(
        !scratch.read(MARKDOWN)?.contains("- **Rule B:**"),
        "no Rule B line while the default applies",
    )?;
    scratch.fragment(
        "x",
        &edit(&decided, "default = ", "rule_b = \"a\"\ndefault = ")?,
    )?;
    fails_with(
        validate(&scratch),
        "a decision-pending entry names no Rule B case (rule_b)",
    )?;
    scratch.fragment(
        "x",
        &edit(
            ONE,
            "who = [\"publisher\", \"bidder\"]",
            "who = [\"publisher\"]\ndefault = \"x\"",
        )?,
    )?;
    fails_with(
        validate(&scratch),
        "only a decision-pending entry has a default",
    )
}

#[test]
fn fields_are_checked() -> TestResult {
    let scratch = Scratch::new()?;
    for (from, to, want) in [
        ("id = \"json.one\"", "id = \"json\"", "the id is not dotted"),
        (
            "status = \"ledgered\"",
            "status = \"accepted\"",
            "unknown status \"accepted\"",
        ),
        (
            "who = [\"publisher\", \"bidder\"]",
            "who = []",
            "who is empty",
        ),
        (
            "who = [\"publisher\", \"bidder\"]",
            "who = [\"user\"]",
            "who \"user\" is none of",
        ),
        (
            "who = [\"publisher\", \"bidder\"]",
            "who = [\"none\", \"bidder\"]",
            "none stands alone",
        ),
        (
            "who = [\"publisher\", \"bidder\"]",
            "who = [\"bidder\", \"bidder\"]",
            "who names bidder twice",
        ),
        (
            "go = \"Go escapes it.\"",
            "go = \"Go\\nescapes it.\"",
            "go holds a line break",
        ),
        (
            "go = \"Go escapes it.\"",
            "go = \"Go escapes `it.\"",
            "go has an odd number",
        ),
        (
            "go = \"Go escapes it.\"",
            "go = \" Go escapes it.\"",
            "go has leading or trailing",
        ),
        ("go = \"Go escapes it.\"", "go = \"\"", "go is empty"),
        ("title = ", "titel = ", "unknown field `titel`"),
        (
            "rust = \"serde_json does not.\"\n",
            "",
            "missing field `rust`",
        ),
        (
            "restore = \"A Formatter; not planned.\"",
            "rule_b = \"a\"",
            "a ledgered entry names no Rule B case",
        ),
        (
            "restore = \"A Formatter; not planned.\"",
            "decision = \"ux\"",
            "is not a DECISIONS.md id",
        ),
    ] {
        scratch.fragment("x", &edit(ONE, from, to)?)?;
        fails_with(validate(&scratch), want)?;
    }
    scratch.fragment("x", "# nothing here\n")?;
    fails_with(
        validate(&scratch),
        "holds no [[difference]] or [[restoration]] table",
    )
}

#[test]
fn an_invalid_ledger_writes_nothing() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment(
        "x",
        &edit(ONE, "category = \"json\"", "category = \"jsn\"")?,
    )?;
    fails_with(validate(&scratch), "unknown category")?;
    check(!scratch.root().join("docs").exists(), "no docs/ written")
}

#[test]
fn the_command_line() -> TestResult {
    let scratch = Scratch::new()?;
    scratch.fragment("x", ONE)?;
    let root = scratch.root().to_string_lossy().into_owned();
    let args = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    check_eq(
        &parse_args(&args(&["--check", "--root", &root]))?,
        &Command::Check(scratch.root().to_path_buf(), Pending::Allowed),
        "--check",
    )?;
    check_eq(
        &parse_args(&args(&["--no-pending", "--write", "--root", &root]))?,
        &Command::Write(scratch.root().to_path_buf(), Pending::Refused),
        "--write --no-pending",
    )?;
    check_eq(&parse_args(&args(&["--help"]))?, &Command::Help, "--help")?;
    fails_with(parse_args(&args(&[])), "give one of --write and --check")?;
    fails_with(
        parse_args(&args(&["--write", "--check"])),
        "give one of --write and --check",
    )?;
    fails_with(parse_args(&args(&["--root"])), "--root needs a directory")?;
    fails_with(
        parse_args(&args(&["--force"])),
        "unknown argument \"--force\"",
    )?;
    let mut out = Vec::new();
    run(&args(&["--write", "--root", &root]), &mut out)?;
    let report = String::from_utf8(out)?;
    check(
        report.starts_with(
            "1 entry from 1 fragment: 0 decision pending, 1 ledgered, 0 Go behaviour kept\n",
        ),
        &report,
    )?;
    check(report.contains("docs/DIFFERENCES.md: written"), &report)?;
    let mut usage = Vec::new();
    run(&args(&["--help"]), &mut usage)?;
    check_eq(String::from_utf8(usage)?.as_str(), USAGE, "the usage")
}
