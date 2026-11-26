//! jsonschema.schema-files: how `new_bidder_params_validator` finds, opens and reports the
//! schema files of a directory, against `testdata/golden/loader.jsonl`, which
//! `testdata/go/loader` writes from the real `openrtb_ext.NewBidderParamsValidator`.
//!
//! Go makes each path absolute with `filepath.Abs` (`os.Getwd`, which gives `$PWD` when `$PWD`
//! names the working directory, then `filepath.Clean`, which drops `..`) and loads the schema
//! through gojsonschema's reference loader for `"file:///" + path`: `url.Parse`, the fragment it
//! cuts at a `#` and `url.QueryUnescape` decide which file Go opens, and `*fs.PathError` words
//! os's failures. The port reads the file at `std::path::absolute`'s path (`getcwd`, `..` kept)
//! and std words the failures. Each case of the golden runs again on a file system laid out the
//! same way, and the port's outcome is checked against the one `PORT` names for it: Go's, or
//! the difference the ledger entry describes.
//!
//! Every case runs with `appnexusalias`, an alias of appnexus, as Prebid registers the aliases
//! of static/bidder-info before it loads the schemas: a file of the alias's name loads, then the
//! alias takes its parent's schema, an absent one too. The port is Go there, and where the files
//! fail in the order of their names and a file has no `.json`.

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        env, fs,
        os::unix::fs::symlink,
        path::PathBuf,
        process::{self, Command},
    };

    use pbs_bidder_params::{
        BidderParamValidator as _, ParamsError, StandardBidderParamValidator,
        new_bidder_params_validator,
    };
    use serde::{Deserialize, Serialize};

    /// The golden `testdata/go/loader` writes.
    const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/loader.jsonl");

    /// How the golden spells the root directory of a case.
    const ROOT: &str = "$ROOT";

    /// The bidder whose schema every case holds.
    const BIDDER: &str = "appnexus";

    /// Go: `alias`, the alias of [`BIDDER`] every case runs with.
    const ALIAS: &str = "appnexusalias";

    /// The variable that names the schema directory [`child_loads`] loads.
    const CHILD_DIR: &str = "PBS_BIDDER_PARAMS_LOADER_DIR";

    /// The variable that names the file [`child_loads`] writes its outcome to.
    const CHILD_OUT: &str = "PBS_BIDDER_PARAMS_LOADER_OUT";

    /// A schema the probe document `1` fails.
    const OBJECT: &str = r#"{"type":"object"}"#;

    /// The probe's text against [`OBJECT`].
    const NOT_AN_OBJECT: &str = "(root): Invalid type. Expected: object, given: integer";

    /// The text of a schema Go and the port both refuse to compile, after a path.
    const BAD_TYPE: &str = "Invalid type. Expected: string/array of strings, given: type";

    /// The port's outcome of a case.
    #[derive(Debug, Clone, Copy)]
    enum Port {
        /// Go's outcome.
        Go,
        /// A failure with this text (`$ROOT` for the root).
        Fails(&'static str),
        /// The validator loads with the appnexus schema of the directory the port is given:
        /// the file's contents and the probe's text, which say it compiled that file, and
        /// [`ALIAS`] has the same two.
        Loads(&'static str, &'static str),
    }

    /// The port's outcome of every case of the golden, by name.
    const PORT: [(&str, Port); 28] = [
        ("loads", Port::Go),
        (
            "missing directory",
            Port::Fails(
                "Failed to read JSON schemas from directory $ROOT/missing. No such file or \
                 directory (os error 2)",
            ),
        ),
        (
            "a file, not a directory",
            Port::Fails(
                "Failed to read JSON schemas from directory $ROOT/f. Not a directory (os error \
                 20)",
            ),
        ),
        ("file name is not a bidder", Port::Go),
        ("schema does not compile", Port::Go),
        // serde_json's text (jsonschema.decode-errors).
        (
            "schema is not JSON",
            Port::Fails(
                "Failed to load json schema at $ROOT/d/appnexus.json: EOF while parsing an \
                 object at line 1 column 1",
            ),
        ),
        (
            "schema is a directory",
            Port::Fails(
                "Failed to load json schema at $ROOT/d/appnexus.json: Is a directory (os error \
                 21)",
            ),
        ),
        (
            "schema is a dangling symlink",
            Port::Fails(
                "Failed to load json schema at $ROOT/d/appnexus.json: No such file or directory \
                 (os error 2)",
            ),
        ),
        ("space", Port::Go),
        ("question mark", Port::Go),
        (
            "dot-dot",
            Port::Fails(
                "Failed to load json schema at $ROOT/sub/../d/appnexus.json: Invalid type. \
                 Expected: string/array of strings, given: type",
            ),
        ),
        ("plus", Port::Loads(OBJECT, NOT_AN_OBJECT)),
        ("plus beside its decoded twin", Port::Loads("{}", "")),
        ("percent escape", Port::Loads(OBJECT, NOT_AN_OBJECT)),
        (
            "percent escape beside its decoded twin",
            Port::Loads("{}", ""),
        ),
        ("bad percent escape", Port::Loads(OBJECT, NOT_AN_OBJECT)),
        (
            "multi-byte text before a bad percent escape",
            Port::Loads(OBJECT, NOT_AN_OBJECT),
        ),
        ("fragment", Port::Loads(OBJECT, NOT_AN_OBJECT)),
        (
            "bad percent escape after a fragment",
            Port::Loads(OBJECT, NOT_AN_OBJECT),
        ),
        ("fragment after a JSON file", Port::Loads("{}", "")),
        ("control character", Port::Loads(OBJECT, NOT_AN_OBJECT)),
        (
            "relative, $PWD names the working directory",
            Port::Fails(
                "Failed to load json schema at $ROOT/real/d/appnexus.json: Invalid type. \
                 Expected: string/array of strings, given: type",
            ),
        ),
        ("relative, $PWD unset", Port::Go),
        ("files fail in the order of their names", Port::Go),
        ("a schema fails before a later file name", Port::Go),
        ("a file without .json", Port::Go),
        ("an alias's own schema gives way to its parent's", Port::Go),
        ("an alias's own schema without its parent's", Port::Go),
    ];

    /// One file system entry of a case, under its root.
    #[derive(Debug, Deserialize)]
    struct Entry {
        path: String,
        /// `file`, `dir` or `symlink`.
        kind: String,
        #[serde(default)]
        contents: String,
        /// A symlink's target, relative to the symlink's directory.
        #[serde(default)]
        target: String,
    }

    /// What one call of the loader gives: the error text, or the appnexus schema's contents and
    /// the text of validating `1` against the schema it compiled ([`probe`]), and the same two
    /// for [`ALIAS`].
    #[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Outcome {
        loaded: bool,
        #[serde(default)]
        error: String,
        #[serde(default)]
        schema: String,
        #[serde(default)]
        probe: String,
        #[serde(default)]
        alias_schema: String,
        #[serde(default)]
        alias_probe: String,
    }

    /// One case of the golden: the files of its root, the call, and Go's outcome.
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        entries: Option<Vec<Entry>>,
        /// The schema directory Prebid is given.
        dir: String,
        /// The working directory of the call under the root, when it is not the test's.
        #[serde(default)]
        cwd: String,
        /// `$PWD` under the root; unset when empty.
        #[serde(default)]
        pwd: String,
        loaded: bool,
        #[serde(default)]
        error: String,
        #[serde(default)]
        schema: String,
        #[serde(default)]
        probe: String,
        #[serde(default)]
        alias_schema: String,
        #[serde(default)]
        alias_probe: String,
    }

    impl Case {
        fn go(&self) -> Outcome {
            Outcome {
                loaded: self.loaded,
                error: self.error.clone(),
                schema: self.schema.clone(),
                probe: self.probe.clone(),
                alias_schema: self.alias_schema.clone(),
                alias_probe: self.alias_probe.clone(),
            }
        }
    }

    /// A new directory for one case, in its physical spelling, removed when dropped.
    #[derive(Debug)]
    struct Root {
        path: PathBuf,
        text: String,
    }

    impl Root {
        fn new(index: usize) -> Result<Self, String> {
            let made = env::temp_dir().join(format!(
                "pbs-bidder-params-loader-{}-{index}",
                process::id()
            ));
            let _cleared = fs::remove_dir_all(&made);
            fs::create_dir_all(&made).map_err(|e| format!("{}: {e}", made.display()))?;
            // The physical spelling, as getcwd gives it in a working directory under the root.
            let path = fs::canonicalize(&made).map_err(|e| format!("{}: {e}", made.display()))?;
            let text = path.display().to_string();
            Ok(Self { path, text })
        }

        fn create(&self, entry: &Entry) -> Result<(), String> {
            let path = self.path.join(&entry.path);
            let parent = path.parent().unwrap_or(&self.path);
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            match entry.kind.as_str() {
                "file" => fs::write(&path, &entry.contents),
                "dir" => fs::create_dir_all(&path),
                "symlink" => symlink(&entry.target, &path),
                other => return Err(format!("{}: unknown entry kind {other}", entry.path)),
            }
            .map_err(|e| format!("{}: {e}", path.display()))
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.path);
        }
    }

    /// Go: `probe`: the text of validating `1` against `name`'s schema, empty when valid, and
    /// `panic` where Go panics.
    fn probe(validator: &StandardBidderParamValidator, name: &str) -> String {
        match validator.validate(name, b"1") {
            Ok(()) => String::new(),
            Err(ParamsError::Panic(_)) => "panic".to_owned(),
            Err(e) => e.to_string(),
        }
    }

    /// The port's outcome of loading `dir`, with the appnexus bidder and [`ALIAS`] known, as
    /// Go's package globals know them.
    fn load(dir: &str) -> Outcome {
        let bidders = BTreeSet::from([BIDDER.to_owned(), ALIAS.to_owned()]);
        let aliases = BTreeMap::from([(ALIAS.to_owned(), BIDDER.to_owned())]);
        match new_bidder_params_validator(dir, &bidders, &aliases) {
            Err(e) => Outcome {
                error: e.to_string(),
                ..Outcome::default()
            },
            Ok(validator) => Outcome {
                loaded: true,
                schema: validator.schema(BIDDER).to_owned(),
                probe: probe(&validator, BIDDER),
                alias_schema: validator.schema(ALIAS).to_owned(),
                alias_probe: probe(&validator, ALIAS),
                ..Outcome::default()
            },
        }
    }

    /// The port's outcome of a case with its own working directory: [`child_loads`] in a
    /// child process of this test binary, which runs in that directory and with the case's
    /// `$PWD`, since a test may change neither for the tests beside it.
    fn load_in_child(root: &Root, case: &Case, dir: &str) -> Result<Outcome, String> {
        let exe = env::current_exe().map_err(|e| e.to_string())?;
        let out = root.path.join("outcome.json");
        let mut command = Command::new(exe);
        let _configured = command
            .args(["tests::child_loads", "--exact", "--ignored"])
            .current_dir(root.path.join(&case.cwd))
            .env(CHILD_DIR, dir)
            .env(CHILD_OUT, &out);
        let _pwd = if case.pwd.is_empty() {
            command.env_remove("PWD")
        } else {
            command.env("PWD", root.path.join(&case.pwd))
        };
        let output = command.output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "{}: the child process failed: {}{}",
                case.name,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let text = fs::read_to_string(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", out.display()))
    }

    /// The child process of [`load_in_child`]: loads the directory [`CHILD_DIR`] names and
    /// writes the outcome to the file [`CHILD_OUT`] names. Without them it does nothing.
    #[test]
    #[ignore = "schema_files_open_by_path runs it in a child process"]
    fn child_loads() -> Result<(), String> {
        let (Some(dir), Some(out)) = (env::var_os(CHILD_DIR), env::var_os(CHILD_OUT)) else {
            return Ok(());
        };
        let outcome = load(&dir.to_string_lossy());
        let text = serde_json::to_string(&outcome).map_err(|e| e.to_string())?;
        fs::write(out, text).map_err(|e| e.to_string())
    }

    /// The port's outcome of `case` on a new root laid out as the case says, the root spelled
    /// [`ROOT`].
    fn run(index: usize, case: &Case) -> Result<Outcome, String> {
        let root = Root::new(index)?;
        for entry in case.entries.iter().flatten() {
            root.create(entry)?;
        }
        let dir = case.dir.replace(ROOT, &root.text);
        let mut outcome = if case.cwd.is_empty() {
            load(&dir)
        } else {
            load_in_child(&root, case, &dir)?
        };
        outcome.error = outcome.error.replace(&root.text, ROOT);
        Ok(outcome)
    }

    /// The cases of the golden.
    fn golden() -> Result<Vec<Case>, String> {
        let text = fs::read_to_string(GOLDEN).map_err(|e| format!("{GOLDEN}: {e}"))?;
        text.lines()
            .map(|line| serde_json::from_str(line).map_err(|e| format!("{e}: {line}")))
            .collect()
    }

    /// The outcome [`PORT`] names for a case whose Go outcome is `go`.
    fn expected(port: Port, go: &Outcome) -> Outcome {
        match port {
            Port::Go => go.clone(),
            Port::Fails(error) => Outcome {
                error: error.to_owned(),
                ..Outcome::default()
            },
            Port::Loads(schema, probe) => Outcome {
                loaded: true,
                schema: schema.to_owned(),
                probe: probe.to_owned(),
                alias_schema: schema.to_owned(),
                alias_probe: probe.to_owned(),
                ..Outcome::default()
            },
        }
    }

    /// jsonschema.schema-files: Go's outcomes are the golden's, and the port's are Go's but
    /// where the URL Go loads through, `filepath.Abs` or os's error texts make them differ.
    // Go: openrtb_ext/bidders.go:803-832 (NewBidderParamsValidator), gojsonschema v1.2.0
    // jsonLoader.go (jsonReferenceLoader.LoadJSON and loadFromFile) and schemaPool.go
    // (GetDocument), net/url/url.go:88 and 398-412 (QueryUnescape, Parse), os/getwd.go:36-64
    // (Getwd), path/filepath/path.go:55 and 161-174 (Clean, Abs)
    #[test]
    fn schema_files_open_by_path() -> Result<(), String> {
        let cases = golden()?;
        let named: BTreeSet<&str> = cases.iter().map(|case| case.name.as_str()).collect();
        let listed: BTreeSet<&str> = PORT.iter().map(|(name, _)| *name).collect();
        if named != listed || named.len() != cases.len() {
            return Err(format!(
                "the golden's cases {named:?} are not the ones PORT lists, {listed:?}"
            ));
        }
        let mut failures = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let port = PORT
                .iter()
                .find(|(name, _)| *name == case.name)
                .map_or(Port::Go, |(_, port)| *port);
            let go = case.go();
            let want = expected(port, &go);
            if !matches!(port, Port::Go) && want == go {
                failures.push(format!(
                    "{}: PORT names a difference, but Go's outcome is the same: {go:?}",
                    case.name
                ));
            }
            let got = run(index, case)?;
            if got != want {
                failures.push(format!(
                    "{}:\n  Go   {go:?}\n  want {want:?}\n  port {got:?}",
                    case.name
                ));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    /// The Go side of the difference, read off the golden: Go starts in none of the
    /// directories whose path holds `+`, `%41`, a bad `%` escape (after a `#` too), `#` or a
    /// control character unless another file answers to the name it decodes, prints `$PWD`'s
    /// spelling of the working directory and drops `..`, and words os's errors as
    /// `*fs.PathError`.
    // Go: openrtb_ext/bidders.go:819-827, gojsonschema v1.2.0 jsonLoader.go (LoadJSON),
    // net/url/url.go:39, 68 and 398-412 (Error, EscapeError, Parse), os/getwd.go:36-52,
    // io/fs/fs.go:267 (PathError)
    #[test]
    fn go_opens_a_file_url() -> Result<(), String> {
        let cases = golden()?;
        let go: BTreeMap<&str, Outcome> = cases
            .iter()
            .map(|case| (case.name.as_str(), case.go()))
            .collect();
        let at = |name: &str| format!("Failed to load json schema at $ROOT/{name}/appnexus.json: ");
        let wants = [
            (
                "missing directory",
                "Failed to read JSON schemas from directory $ROOT/missing. open $ROOT/missing: \
                 no such file or directory"
                    .to_owned(),
            ),
            (
                "schema is a directory",
                format!("{}read /$ROOT/d/appnexus.json: is a directory", at("d")),
            ),
            (
                "dot-dot",
                format!("Failed to load json schema at $ROOT/d/appnexus.json: {BAD_TYPE}"),
            ),
            (
                "plus",
                format!(
                    "{}open /$ROOT/a b/appnexus.json: no such file or directory",
                    at("a+b")
                ),
            ),
            (
                "percent escape",
                format!(
                    "{}open /$ROOT/aA/appnexus.json: no such file or directory",
                    at("a%41")
                ),
            ),
            (
                "bad percent escape",
                format!(
                    "{}parse \"file:///$ROOT/a%zz/appnexus.json\": invalid URL escape \"%zz\"",
                    at("a%zz")
                ),
            ),
            (
                "multi-byte text before a bad percent escape",
                format!(
                    "{}parse \"file:///$ROOT/\u{e9}%zz/appnexus.json\": invalid URL escape \"%zz\"",
                    at("\u{e9}%zz")
                ),
            ),
            (
                "fragment",
                format!("{}open /$ROOT/a: no such file or directory", at("a#b")),
            ),
            (
                "bad percent escape after a fragment",
                format!(
                    "{}parse \"file:///$ROOT/a#b%zz/appnexus.json\": invalid URL escape \"%zz\"",
                    at("a#b%zz")
                ),
            ),
            (
                "control character",
                format!(
                    "{}parse \"file:///$ROOT/a\\tb/appnexus.json\": net/url: invalid control \
                     character in URL",
                    at("a\tb")
                ),
            ),
            (
                "relative, $PWD names the working directory",
                format!("{}{BAD_TYPE}", at("link/d")),
            ),
        ];
        let mut failures = Vec::new();
        for (name, want) in wants {
            match go.get(name) {
                Some(outcome) if !outcome.loaded && outcome.error == want => {}
                other => failures.push(format!("{name}: Go {other:?}, want the error {want}")),
            }
        }
        // Where another file answers to the decoded name, Go compiles that file's schema and
        // still returns the directory's own file from Schema.
        for name in [
            "plus beside its decoded twin",
            "percent escape beside its decoded twin",
            "fragment after a JSON file",
        ] {
            match go.get(name) {
                Some(outcome)
                    if outcome.loaded
                        && outcome.schema == "{}"
                        && outcome.probe == NOT_AN_OBJECT => {}
                other => failures.push(format!(
                    "{name}: Go {other:?}, want the twin's schema compiled"
                )),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}
