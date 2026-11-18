//! `pbs-devtools adapter-manifest`: the Go adapter fixture manifest,
//! `testdata/go_fixture_manifest.json`.
//!
//! For every `adapterstest.RunJSONBidderTest(t, dir, bidder)` call in `adapters/*/*_test.go`, the
//! generator finds how `bidder` was built — a `Builder(name, config.Adapter{...},
//! config.Server{...})` call, possibly through local variables or a helper function in the same
//! package, or a struct literal — and resolves every argument to a literal: string and raw
//! string literals, `+` concatenation, package constants and variables, and `openrtb_ext.BidderXxx`
//! constants (read from `openrtb_ext/*.go`). Statements that change the bidder after it is built
//! must be accounted for by the hooks overlay ([`crate::hooks`]). Any run it cannot resolve fails
//! generation, as does a change in the expected totals.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    ops::Range,
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    error::{Result, ToolError},
    fixtures,
    golex::Token,
    goparse::{Expr, File, Func, IfParts, StmtKind, render},
    hooks::{HOOKS_PATH, Hooks, HooksFile, RunHooks},
    repo::Repo,
};

/// Workspace-relative path of the manifest.
pub const MANIFEST_PATH: &str = "testdata/go_fixture_manifest.json";

/// The generated manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// What produced the file.
    pub generator: String,
    /// The Go commit the manifest was generated from.
    pub go_commit: String,
    /// How to read the file.
    pub notes: Vec<String>,
    /// Counts over all runs.
    pub totals: Totals,
    /// Hooks that apply to every run (from the overlay).
    pub global_hooks: Hooks,
    /// `.json` files under `adapters/` that no run executes, repository-relative.
    pub never_run_json_files: Vec<String>,
    /// One entry per `RunJSONBidderTest` call, sorted by package, file, then position.
    pub runs: Vec<Run>,
}

/// Counts the manifest asserts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    /// Number of `RunJSONBidderTest` calls.
    pub runs: u64,
    /// Fixture executions per directory kind, summed over runs.
    pub fixture_runs: BTreeMap<String, u64>,
    /// Sum of `fixture_runs`.
    pub fixture_runs_total: u64,
    /// Distinct fixture files executed by at least one run.
    pub distinct_fixture_files: u64,
}

/// One `RunJSONBidderTest` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// The adapter directory under `adapters/`.
    pub go_pkg: String,
    /// The Go package name (the qualifier in Go type names and error messages).
    pub go_package: String,
    /// The test function calling `RunJSONBidderTest`.
    pub go_test: String,
    /// The file declaring `go_test`, repository-relative.
    pub go_test_file: String,
    /// The fixture root directory, repository-relative.
    pub fixture_root: String,
    /// The `BidderName` passed to `Builder`; `None` for struct-literal construction.
    pub bidder_name: Option<String>,
    /// How the bidder is constructed.
    pub construct: Construct,
    /// The function whose body holds the `Builder` call or struct literal.
    pub built_by: String,
    /// `config.Adapter` passed to `Builder` (all fields, empty when unset).
    pub adapter: Option<AdapterConfig>,
    /// `config.Server` passed to `Builder`.
    pub server: Option<ServerConfig>,
    /// The struct literal for [`Construct::Direct`].
    pub direct: Option<Direct>,
    /// Fixture files the run executes, per directory kind.
    pub fixtures: BTreeMap<String, u64>,
    /// Statements that change state around construction (accounted for by `hooks`).
    pub setup: Vec<String>,
    /// Hooks from the overlay.
    pub hooks: Hooks,
}

/// How a run's bidder is constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Construct {
    /// Through the package's `Builder`.
    Builder,
    /// As a struct literal of the adapter type.
    Direct,
}

/// `config.Adapter` as passed to a builder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterConfig {
    /// `Endpoint`.
    pub endpoint: String,
    /// `ExtraAdapterInfo`.
    pub extra_info: String,
    /// `PlatformID`.
    pub platform_id: String,
    /// `AppSecret`.
    pub app_secret: String,
    /// `XAPI`.
    pub xapi: XapiConfig,
}

/// `config.AdapterXAPI`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XapiConfig {
    /// `Username`.
    pub username: String,
    /// `Password`.
    pub password: String,
    /// `Tracker`.
    pub tracker: String,
}

/// `config.Server` as passed to a builder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// `ExternalUrl`.
    pub external_url: String,
    /// `GvlID`.
    pub gvl_id: i64,
    /// `DataCenter`.
    pub datacenter: String,
}

/// A struct literal that builds the bidder directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Direct {
    /// The literal's type as written (`YieldlabAdapter`).
    #[serde(rename = "type")]
    pub ty: String,
    /// Fields whose values resolved to strings.
    pub fields: BTreeMap<String, String>,
    /// Fields with values the generator cannot evaluate (functions), as Go source; the overlay
    /// must supply them as hooks.
    pub opaque_fields: BTreeMap<String, String>,
}

/// Generates the manifest from the Go sources and the hooks overlay.
///
/// # Errors
///
/// Fails, listing every problem, when a run cannot be resolved, a fixture root is missing, the
/// overlay does not match the detected setup statements or test files, or the totals differ
/// from the overlay's `expected_totals`.
pub fn generate(repo: &Repo, go_commit: &str) -> Result<Manifest> {
    let hooks = HooksFile::load(&repo.path(HOOKS_PATH))?;
    let consts = bidder_constants(repo)?;
    let mut errors = Vec::new();
    let mut runs = Vec::new();

    let adapters = repo.go_path("adapters");
    let mut dirs: Vec<String> = Vec::new();
    for entry in fs::read_dir(&adapters).map_err(|e| ToolError::io(&adapters, &e))? {
        let entry = entry.map_err(|e| ToolError::io(&adapters, &e))?;
        if entry
            .file_type()
            .map_err(|e| ToolError::io(&entry.path(), &e))?
            .is_dir()
            && let Ok(name) = entry.file_name().into_string()
        {
            dirs.push(name);
        }
    }
    dirs.sort();
    for dir in &dirs {
        let pkg = Package::load(repo, dir)?;
        pkg.runs(&consts, &mut runs, &mut errors);
    }

    let mut executed = BTreeSet::new();
    let mut fixture_runs: BTreeMap<String, u64> = fixtures::DIR_KINDS
        .iter()
        .map(|k| ((*k).to_owned(), 0))
        .collect();
    for run in &mut runs {
        let root = repo.go_path(&run.fixture_root);
        if !root.is_dir() {
            errors.push(format!(
                "{}/{}: fixture root {} does not exist",
                run.go_pkg, run.go_test, run.fixture_root
            ));
            continue;
        }
        let found = fixtures::walk(&root)?;
        if found.is_empty() {
            errors.push(format!(
                "{}/{}: fixture root {} has no fixtures",
                run.go_pkg, run.go_test, run.fixture_root
            ));
        }
        run.fixtures = fixtures::count(&found);
        for (kind, n) in &run.fixtures {
            *fixture_runs.entry(kind.clone()).or_insert(0) += n;
        }
        executed.extend(found.into_iter().map(|f| f.path));
    }

    let mut never_run = Vec::new();
    for path in fixtures::all_json(&adapters)? {
        if !executed.contains(&path) {
            let rel = path.strip_prefix(repo.go_root()).unwrap_or(&path);
            never_run.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }

    apply_hooks(repo, &hooks, &mut runs, &mut errors)?;

    let totals = Totals {
        runs: runs.len() as u64,
        fixture_runs_total: fixture_runs.values().sum(),
        fixture_runs,
        distinct_fixture_files: executed.len() as u64,
    };
    if totals != hooks.expected_totals {
        errors.push(format!(
            "totals changed: expected_totals in {HOOKS_PATH} is {}, the Go tree now gives {}. \
             Review the new or removed adapters and fixtures, then update expected_totals.",
            serde_json::to_string(&hooks.expected_totals)?,
            serde_json::to_string(&totals)?
        ));
    }

    if !errors.is_empty() {
        return Err(ToolError::new(format!(
            "adapter-manifest found {} problem(s):\n  - {}",
            errors.len(),
            errors.join("\n  - ")
        )));
    }

    Ok(Manifest {
        generator: "pbs-devtools adapter-manifest (rust/crates/devtools)".to_owned(),
        go_commit: go_commit.to_owned(),
        notes: vec![
            "One run per adapterstest.RunJSONBidderTest call in adapters/*/*_test.go.".to_owned(),
            "fixture_root is walked like test_json.go: a .json file runs when its parent \
             directory is named amp, exemplary, supplemental, video or videosupplemental."
                .to_owned(),
            "adapter/server are the config.Adapter/config.Server literals passed to Builder, \
             every field present (\"\"/0 when unset)."
                .to_owned(),
            "setup lists statements around construction that change state; hooks (from \
             go_fixture_hooks.json) give their effect in a fixed vocabulary, and global_hooks \
             apply to every run."
                .to_owned(),
        ],
        totals,
        global_hooks: hooks.global_hooks,
        never_run_json_files: never_run,
        runs,
    })
}

fn apply_hooks(
    repo: &Repo,
    hooks: &HooksFile,
    runs: &mut [Run],
    errors: &mut Vec<String>,
) -> Result<()> {
    let mut matched = BTreeSet::new();
    for run in runs.iter_mut() {
        let key = format!("{}/{}", run.go_pkg, run.go_test);
        let opaque: Vec<String> = run
            .direct
            .as_ref()
            .map_or_else(Vec::new, |d| d.opaque_fields.keys().cloned().collect());
        let entry = hooks.runs.get(&key);
        let needs_entry = !run.setup.is_empty() || !opaque.is_empty();
        let sha = file_sha256(repo, &run.go_test_file)?;
        let Some(entry) = entry else {
            if needs_entry {
                let suggestion = RunHooks {
                    hooks: Hooks::default(),
                    setup: run.setup.clone(),
                    opaque_fields: opaque,
                    go_test_file_sha256: sha,
                    notes: String::new(),
                };
                errors.push(format!(
                    "{key}: construction involves state the manifest cannot express; add an \
                     entry to {HOOKS_PATH} after reading {}: \"{key}\": {}",
                    run.go_test_file,
                    serde_json::to_string(&suggestion)?
                ));
            }
            continue;
        };
        let _ = matched.insert(key.clone());
        if entry.setup != run.setup {
            errors.push(format!(
                "{key}: setup statements changed; the overlay has {:?}, Go now has {:?}. \
                 Re-review the hooks.",
                entry.setup, run.setup
            ));
        }
        if entry.opaque_fields != opaque {
            errors.push(format!(
                "{key}: opaque struct fields changed; the overlay has {:?}, Go now has {opaque:?}.",
                entry.opaque_fields
            ));
        }
        if entry.go_test_file_sha256 != sha {
            errors.push(format!(
                "{key}: {} changed since the hooks were reviewed (sha256 now {sha}); re-check \
                 the hooks against the Go test, then update go_test_file_sha256.",
                run.go_test_file
            ));
        }
        run.hooks = entry.hooks.clone();
    }
    for key in hooks.runs.keys() {
        if !matched.contains(key) {
            errors.push(format!("{key}: overlay entry matches no run (stale?)"));
        }
    }
    Ok(())
}

fn file_sha256(repo: &Repo, rel: &str) -> Result<String> {
    let path = repo.go_path(rel);
    let bytes = fs::read(&path).map_err(|e| ToolError::io(&path, &e))?;
    let mut hex = String::with_capacity(64);
    for b in Sha256::digest(&bytes) {
        write!(hex, "{b:02x}").map_err(|e| ToolError::new(format!("formatting a digest: {e}")))?;
    }
    Ok(hex)
}

/// Every `BidderName` constant declared in `openrtb_ext`, by Go name.
///
/// # Errors
///
/// Fails when the package cannot be read or parsed.
pub fn bidder_constants(repo: &Repo) -> Result<BTreeMap<String, String>> {
    let files = read_go_files(repo, "openrtb_ext", false)?;
    let out: BTreeMap<String, String> = files
        .iter()
        .flat_map(|file| &file.values)
        .filter_map(|v| match &v.value {
            Some(Expr::Str(s)) if v.is_const && v.ty == "BidderName" => {
                Some((v.name.clone(), s.clone()))
            }
            _ => None,
        })
        .collect();
    if out.is_empty() {
        return Err(ToolError::new(
            "no BidderName constants found in openrtb_ext",
        ));
    }
    Ok(out)
}

fn read_go_files(repo: &Repo, rel_dir: &str, include_tests: bool) -> Result<Vec<File>> {
    let dir = repo.go_path(rel_dir);
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| ToolError::io(&dir, &e))? {
        let entry = entry.map_err(|e| ToolError::io(&dir, &e))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // go/build: `.go` files (symlinks to files included), ignoring names starting with `_`
        // or `.`.
        let is_go = Path::new(&name).extension().is_some_and(|ext| ext == "go")
            && !name.starts_with(['_', '.'])
            && !entry.path().is_dir();
        if is_go && (include_tests || !name.ends_with("_test.go")) {
            names.push(name);
        }
    }
    names.sort();
    let mut files = Vec::with_capacity(names.len());
    for name in names {
        let path = dir.join(&name);
        let src = fs::read_to_string(&path).map_err(|e| ToolError::io(&path, &e))?;
        files.push(parse_go_file(&format!("{rel_dir}/{name}"), src)?);
    }
    Ok(files)
}

/// Parses one Go file (`rel` is its repository-relative path), refusing files with build
/// constraints: the generator reads every file, so one `go test` may skip would make the manifest
/// depend on the build environment.
fn parse_go_file(rel: &str, src: String) -> Result<File> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if let Some(why) = build_constraint(name, &src) {
        return Err(ToolError::new(format!(
            "{rel}: has a build constraint ({why}), which the generator does not evaluate"
        )));
    }
    File::parse(rel, src)
}

// Go: go/build/syslist.go knownOS and knownArch.
const KNOWN_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];
const KNOWN_ARCH: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];

/// Why `go/build` would match the file against the build environment, if it would: a
/// `//go:build` or `// +build` line in the header before the package clause, or a `_GOOS`,
/// `_GOARCH` or `_GOOS_GOARCH` file-name suffix.
fn build_constraint(name: &str, src: &str) -> Option<String> {
    // Go: go/build/build.go goodOSArchFile.
    let stem = name.split('.').next().unwrap_or_default();
    if let Some((_, after_first)) = stem.split_once('_') {
        let mut parts: Vec<&str> = after_first.split('_').collect();
        if parts.last() == Some(&"test") {
            let _ = parts.pop();
        }
        let os_arch = match parts.as_slice() {
            [.., os, arch] if KNOWN_OS.contains(os) && KNOWN_ARCH.contains(arch) => true,
            [.., last] => KNOWN_OS.contains(last) || KNOWN_ARCH.contains(last),
            [] => false,
        };
        if os_arch {
            return Some("a GOOS/GOARCH file-name suffix".to_owned());
        }
    }
    // Go: go/build/build.go shouldBuild reads constraints from the leading comments only.
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("//go:build") || line.starts_with("// +build") {
            return Some(format!("`{line}`"));
        }
        if line.starts_with("package") {
            break;
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Package model and evaluation

struct Package {
    dir: String,
    files: Vec<File>,
}

impl Package {
    fn load(repo: &Repo, dir: &str) -> Result<Self> {
        let files = read_go_files(repo, &format!("adapters/{dir}"), true)?;
        Ok(Self {
            dir: dir.to_owned(),
            files,
        })
    }

    fn package_name(&self) -> String {
        self.files
            .iter()
            .find(|f| !f.path.ends_with("_test.go"))
            .or_else(|| self.files.first())
            .map_or_else(String::new, |f| {
                f.package.trim_end_matches("_test").to_owned()
            })
    }

    fn func(&self, name: &str) -> Option<(usize, &Func)> {
        self.files.iter().enumerate().find_map(|(i, f)| {
            f.funcs
                .iter()
                .find(|func| func.name == name)
                .map(|func| (i, func))
        })
    }

    /// The package-level declaration of `name`: its file, its value, and whether it is a `var`.
    fn value(&self, name: &str) -> Option<(usize, &Expr, bool)> {
        self.files.iter().enumerate().find_map(|(i, f)| {
            f.values
                .iter()
                .find(|v| v.name == name)
                .and_then(|v| v.value.as_ref().map(|e| (i, e, !v.is_const)))
        })
    }

    /// Whether some function or initializer of the package assigns `name`.
    fn is_assigned(&self, name: &str) -> bool {
        self.files.iter().any(|f| f.assigned.contains(name))
    }

    fn runs(
        &self,
        consts: &BTreeMap<String, String>,
        runs: &mut Vec<Run>,
        errors: &mut Vec<String>,
    ) {
        let first_run = runs.len();
        for (fi, file) in self.files.iter().enumerate() {
            let is_test_file = file.path.ends_with("_test.go");
            let total = count_run_refs(file.toks(0..file.tokens.len()));
            let mut followed = 0;
            for func in &file.funcs {
                for (si, stmt) in func.body.iter().enumerate() {
                    let mentions = count_run_refs(file.toks(stmt.tokens.clone()));
                    if mentions == 0 || !is_test_file {
                        continue;
                    }
                    followed += mentions;
                    let ctx = format!("{}:{}", file.path, func.name);
                    if !func.is_go_test {
                        // A helper or a function `go test` does not run: the run (if any) belongs
                        // to its callers, which the generator does not trace.
                        errors.push(format!(
                            "{ctx}: RunJSONBidderTest is called from a function that is not a Go \
                             test (func TestXxx(t *testing.T)); the generator does not follow it"
                        ));
                        continue;
                    }
                    match call_site(file, func, si) {
                        Some((scope, call_index, args)) => {
                            let mut ev = Eval::new(self, consts);
                            match ev.run(fi, &scope, call_index, &args) {
                                Ok(run) => runs.push(run),
                                Err(e) => errors.push(format!("{ctx}: {e}")),
                            }
                        }
                        None => errors.push(format!(
                            "{ctx}: RunJSONBidderTest is called in a form the generator does not \
                             follow (only a plain statement, or a statement in the then-block of \
                             `if x, err := f(...); err == nil {{ ... }}`)"
                        )),
                    }
                }
            }
            if total != followed {
                errors.push(format!(
                    "{}: RunJSONBidderTest is referenced {} time(s) outside the statements of a \
                     top-level test function (a method, function literal, package-level \
                     declaration or non-test file); the generator does not follow it",
                    file.path,
                    total - followed
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for run in runs.get(first_run..).unwrap_or_default() {
            if !seen.insert(run.go_test.clone()) {
                errors.push(format!(
                    "{}:{}: more than one RunJSONBidderTest call",
                    run.go_test_file, run.go_test
                ));
            }
        }
    }
}

/// Identifier tokens referring to `RunJSONBidderTest` (its declaration in `adapterstest` is not a
/// reference).
fn count_run_refs(toks: &[Token]) -> usize {
    let mut prev: Option<&Token> = None;
    let mut n = 0;
    for t in toks {
        if t.is_ident() && t.text == "RunJSONBidderTest" && !prev.is_some_and(|p| p.is("func")) {
            n += 1;
        }
        prev = Some(t);
    }
    n
}

/// Finds the `RunJSONBidderTest` call in statement `si` of `func` and returns the scope it runs
/// in (a copy of `func` whose body is the statements visible at the call), the call's index in
/// that body, and its arguments.
///
/// Two forms are followed: a plain call statement, and a call statement in the then-block of
/// `if x, err := f(...); err == nil { ... }` (`if bidder, err := Builder(...); err == nil`),
/// whose init statement is then in scope. The condition must be `<name> == nil` for a name the
/// init binds, so the then-block runs exactly when construction succeeds; any other `if` could
/// skip the call at run time and is not followed.
fn call_site(file: &File, func: &Func, si: usize) -> Option<(Func, usize, Vec<Expr>)> {
    let stmt = func.body.get(si)?;
    let prefix = func.body.get(..si)?.to_vec();
    let (body, args) = match &stmt.kind {
        StmtKind::Expr(e) => (prefix, run_call_args(e)?),
        StmtKind::If => {
            let IfParts { init, cond, then } = file.if_parts(stmt.tokens.clone())?;
            let bound = match init.as_ref().map(|s| &s.kind) {
                Some(StmtKind::Bind { names, values, .. }) if values.len() == 1 => names,
                _ => return None,
            };
            let checks_bound_error = matches!(file.toks(cond), [x, eq, nil]
                if x.is_ident() && eq.is("==") && nil.text == "nil"
                    && bound.iter().skip(1).any(|n| *n == x.text));
            if !checks_bound_error {
                return None;
            }
            let pos = then
                .iter()
                .position(|s| matches!(&s.kind, StmtKind::Expr(e) if run_call_args(e).is_some()))?;
            let StmtKind::Expr(call) = &then.get(pos)?.kind else {
                return None;
            };
            let args = run_call_args(call)?;
            let mut body = prefix;
            body.extend(init);
            body.extend(then.into_iter().take(pos));
            (body, args)
        }
        StmtKind::Bind { .. } | StmtKind::Return(_) | StmtKind::Other => return None,
    };
    let index = body.len();
    Some((
        Func {
            name: func.name.clone(),
            params: func.params.clone(),
            body,
            is_go_test: func.is_go_test,
        },
        index,
        args,
    ))
}

fn run_call_args(e: &Expr) -> Option<Vec<Expr>> {
    let Expr::Call { func, args } = e else {
        return None;
    };
    matches!(
        render(func).as_str(),
        "adapterstest.RunJSONBidderTest" | "RunJSONBidderTest"
    )
    .then(|| args.clone())
}

/// A value the evaluator reduced an expression to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Val {
    Str(String),
    Int(i64),
    Composite {
        ty: String,
        fields: Vec<(String, Self)>,
    },
    Call {
        callee: String,
        args: Vec<Self>,
    },
    /// Element `index` of a multi-value call result.
    Tuple(Box<Self>, usize),
    /// Source text of something that could not be evaluated.
    Opaque(String),
}

impl Val {
    fn describe(&self) -> String {
        match self {
            Self::Str(s) => format!("{s:?}"),
            Self::Int(i) => i.to_string(),
            Self::Composite { ty, fields } => {
                let f: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.describe()))
                    .collect();
                format!("{ty}{{{}}}", f.join(", "))
            }
            Self::Call { callee, args } => {
                let a: Vec<String> = args.iter().map(Self::describe).collect();
                format!("{callee}({})", a.join(", "))
            }
            Self::Tuple(call, i) => format!("{}[{i}]", call.describe()),
            Self::Opaque(s) => s.clone(),
        }
    }
}

#[derive(Clone)]
struct Frame<'a> {
    file: usize,
    func: Option<&'a Func>,
    /// Statements of `func` before this index are in scope.
    upto: usize,
    params: BTreeMap<String, Val>,
}

struct Eval<'a> {
    pkg: &'a Package,
    consts: &'a BTreeMap<String, String>,
    /// Binding statements consumed while evaluating: (file, function, statement).
    used: BTreeSet<(usize, String, usize)>,
    /// Function frames entered (the test function first), for setup detection.
    frames: Vec<Frame<'a>>,
    /// Package variables read through their declaration although some function assigns them.
    mutable_vars: BTreeSet<String>,
    depth: usize,
}

const MAX_DEPTH: usize = 64;

struct Construction {
    construct: Construct,
    bidder_name: Option<String>,
    adapter: Option<AdapterConfig>,
    server: Option<ServerConfig>,
    direct: Option<Direct>,
    built_by: String,
}

impl<'a> Eval<'a> {
    const fn new(pkg: &'a Package, consts: &'a BTreeMap<String, String>) -> Self {
        Self {
            pkg,
            consts,
            used: BTreeSet::new(),
            frames: Vec::new(),
            mutable_vars: BTreeSet::new(),
            depth: 0,
        }
    }

    fn run(&mut self, fi: usize, func: &'a Func, si: usize, args: &[Expr]) -> Result<Run> {
        let [_, dir, bidder] = args else {
            return Err(ToolError::new(format!(
                "RunJSONBidderTest has {} arguments, want 3",
                args.len()
            )));
        };
        let frame = Frame {
            file: fi,
            func: Some(func),
            upto: si,
            params: BTreeMap::new(),
        };
        self.frames.push(frame.clone());
        let dir = string(self.eval(dir, &frame), "fixture directory")?;
        let bidder = self.eval(bidder, &frame);
        let built = self.construct(bidder, &frame, 0)?;
        if !self.mutable_vars.is_empty() {
            return Err(ToolError::new(format!(
                "the construction reads package variable(s) {:?} that a function or initializer \
                 of the package assigns, so the value when the test runs may not be the \
                 declaration",
                self.mutable_vars
            )));
        }
        let file = self
            .pkg
            .files
            .get(fi)
            .ok_or_else(|| ToolError::new("file index out of range"))?;
        Ok(Run {
            go_pkg: self.pkg.dir.clone(),
            go_package: self.pkg.package_name(),
            go_test: func.name.clone(),
            go_test_file: file.path.clone(),
            fixture_root: format!("adapters/{}/{}", self.pkg.dir, clean_rel(&dir)?),
            bidder_name: built.bidder_name,
            construct: built.construct,
            built_by: built.built_by,
            adapter: built.adapter,
            server: built.server,
            direct: built.direct,
            fixtures: BTreeMap::new(),
            setup: self.setup(),
            hooks: Hooks::default(),
        })
    }

    fn construct(&mut self, value: Val, frame: &Frame<'a>, nesting: usize) -> Result<Construction> {
        if nesting > 8 {
            return Err(ToolError::new("helper functions nest too deeply"));
        }
        let built_by = frame.func.map_or_else(String::new, |f| f.name.clone());
        match value {
            Val::Tuple(call, 0) => {
                if let Val::Call { callee, args } = call.as_ref()
                    && is_builder(callee)
                {
                    builder_args(args.clone(), built_by)
                } else {
                    Err(ToolError::new(format!(
                        "bidder comes from {}, not Builder",
                        call.describe()
                    )))
                }
            }
            Val::Call { callee, args } => {
                let Some((fi, helper)) = self.pkg.func(&callee) else {
                    return Err(ToolError::new(format!(
                        "bidder comes from {callee}(...), which is not a function of this package"
                    )));
                };
                let returns: Vec<(usize, &Vec<Expr>)> = helper
                    .body
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        if let StmtKind::Return(values) = &s.kind {
                            Some((i, values))
                        } else {
                            None
                        }
                    })
                    .collect();
                let [(ri, ret)] = returns.as_slice() else {
                    return Err(ToolError::new(format!(
                        "helper {callee} has {} top-level returns, want 1",
                        returns.len()
                    )));
                };
                let params: BTreeMap<String, Val> =
                    helper.params.iter().cloned().zip(args).collect();
                let hf = Frame {
                    file: fi,
                    func: Some(helper),
                    upto: *ri,
                    params,
                };
                self.frames.push(hf.clone());
                let first = ret
                    .first()
                    .ok_or_else(|| ToolError::new(format!("helper {callee} returns nothing")))?;
                let returned = self.eval(first, &hf);
                self.construct(returned, &hf, nesting + 1)
            }
            Val::Composite { ty, fields } => {
                let mut direct = Direct {
                    ty: ty.trim_start_matches('&').to_owned(),
                    fields: BTreeMap::new(),
                    opaque_fields: BTreeMap::new(),
                };
                for (k, field) in fields {
                    let previous = if let Val::Str(s) = field {
                        direct.fields.insert(k.clone(), s)
                    } else {
                        direct.opaque_fields.insert(k.clone(), field.describe())
                    };
                    if previous.is_some() {
                        return Err(ToolError::new(format!("{ty} literal sets {k} twice")));
                    }
                }
                Ok(Construction {
                    construct: Construct::Direct,
                    bidder_name: None,
                    adapter: None,
                    server: None,
                    direct: Some(direct),
                    built_by,
                })
            }
            other @ (Val::Str(_) | Val::Int(_) | Val::Tuple(..) | Val::Opaque(_)) => {
                Err(ToolError::new(format!(
                    "cannot tell how the bidder {} is built",
                    other.describe()
                )))
            }
        }
    }

    fn eval(&mut self, e: &Expr, frame: &Frame<'a>) -> Val {
        self.depth += 1;
        let v = if self.depth > MAX_DEPTH {
            Val::Opaque(render(e))
        } else {
            self.eval_inner(e, frame)
        };
        self.depth -= 1;
        v
    }

    fn eval_inner(&mut self, e: &Expr, frame: &Frame<'a>) -> Val {
        match e {
            Expr::Str(s) => Val::Str(s.clone()),
            Expr::Int(i) => Val::Int(*i),
            Expr::Ident(name) => self.lookup(name, frame),
            Expr::Selector(x, name) => match (x.as_ref(), self.consts.get(name)) {
                (Expr::Ident(pkg), Some(v)) if pkg == "openrtb_ext" => Val::Str(v.clone()),
                _ => Val::Opaque(render(e)),
            },
            Expr::Call { func, args } => {
                let args = args.iter().map(|a| self.eval(a, frame)).collect();
                Val::Call {
                    callee: render(func),
                    args,
                }
            }
            Expr::Composite { ty, elems } => {
                let mut fields = Vec::new();
                for (i, el) in elems.iter().enumerate() {
                    let key = match &el.key {
                        Some(Expr::Ident(k)) => k.clone(),
                        Some(other) => render(other),
                        None => format!("#{i}"),
                    };
                    fields.push((key, self.eval(&el.value, frame)));
                }
                Val::Composite {
                    ty: ty.clone(),
                    fields,
                }
            }
            Expr::Unary { op, expr } if op == "&" => {
                if let Val::Composite { ty, fields } = self.eval(expr, frame) {
                    Val::Composite {
                        ty: format!("&{ty}"),
                        fields,
                    }
                } else {
                    Val::Opaque(render(e))
                }
            }
            Expr::Binary { op, lhs, rhs } if op == "+" => {
                match (self.eval(lhs, frame), self.eval(rhs, frame)) {
                    (Val::Str(a), Val::Str(b)) => Val::Str(a + &b),
                    (Val::Int(a), Val::Int(b)) => a
                        .checked_add(b)
                        .map_or_else(|| Val::Opaque(render(e)), Val::Int),
                    _ => Val::Opaque(render(e)),
                }
            }
            Expr::Unary { .. } | Expr::Binary { .. } | Expr::Other(_) => Val::Opaque(render(e)),
        }
    }

    fn lookup(&mut self, name: &str, frame: &Frame<'a>) -> Val {
        if let Some(func) = frame.func {
            let scope = func.body.get(..frame.upto).unwrap_or_default();
            for (si, stmt) in scope.iter().enumerate().rev() {
                let StmtKind::Bind { names, values, .. } = &stmt.kind else {
                    continue;
                };
                let Some(k) = names.iter().position(|n| n == name) else {
                    continue;
                };
                let _ = self.used.insert((frame.file, func.name.clone(), si));
                let inner = Frame {
                    upto: si,
                    ..frame.clone()
                };
                return if values.len() == names.len() {
                    values
                        .get(k)
                        .map_or_else(|| Val::Opaque(name.to_owned()), |v| self.eval(v, &inner))
                } else if let [single] = values.as_slice() {
                    Val::Tuple(Box::new(self.eval(single, &inner)), k)
                } else {
                    Val::Opaque(name.to_owned())
                };
            }
            if let Some(v) = frame.params.get(name) {
                return v.clone();
            }
        }
        if let Some((fi, value, is_var)) = self.pkg.value(name) {
            if is_var && self.pkg.is_assigned(name) {
                // Another test, init() or helper may have changed it before this test runs.
                let _ = self.mutable_vars.insert(name.to_owned());
            }
            let pf = Frame {
                file: fi,
                func: None,
                upto: 0,
                params: BTreeMap::new(),
            };
            return self.eval(value, &pf);
        }
        Val::Opaque(name.to_owned())
    }

    /// Statements in the entered frames that were not consumed by evaluation and are not
    /// harmless error checks.
    fn setup(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (n, frame) in self.frames.iter().enumerate() {
            let Some(func) = frame.func else { continue };
            let Some(file) = self.pkg.files.get(frame.file) else {
                continue;
            };
            for (si, stmt) in func.body.iter().enumerate().take(frame.upto) {
                if self.used.contains(&(frame.file, func.name.clone(), si))
                    || is_benign(file, &stmt.kind, stmt.tokens.clone())
                    || binds_unread_local(file, func, si)
                {
                    continue;
                }
                let text = file.text(stmt.tokens.clone());
                out.push(if n == 0 {
                    text
                } else {
                    format!("{}: {text}", func.name)
                });
            }
        }
        out
    }
}

fn is_builder(callee: &str) -> bool {
    callee == "Builder" || callee.ends_with(".Builder")
}

fn builder_args(args: Vec<Val>, built_by: String) -> Result<Construction> {
    let [name, adapter, server] = <[Val; 3]>::try_from(args)
        .map_err(|a| ToolError::new(format!("Builder has {} arguments, want 3", a.len())))?;
    let bidder_name = string(name, "bidder name")?;
    let mut cfg = AdapterConfig::default();
    for (k, v) in fields_of(&adapter, "config.Adapter")? {
        match k.as_str() {
            "Endpoint" => cfg.endpoint = string(v, &k)?,
            "ExtraAdapterInfo" => cfg.extra_info = string(v, &k)?,
            "PlatformID" => cfg.platform_id = string(v, &k)?,
            "AppSecret" => cfg.app_secret = string(v, &k)?,
            "XAPI" => {
                for (xk, xv) in fields_of(&v, "config.AdapterXAPI")? {
                    match xk.as_str() {
                        "Username" => cfg.xapi.username = string(xv, &xk)?,
                        "Password" => cfg.xapi.password = string(xv, &xk)?,
                        "Tracker" => cfg.xapi.tracker = string(xv, &xk)?,
                        _ => {
                            return Err(ToolError::new(format!(
                                "unknown config.AdapterXAPI field {xk}"
                            )));
                        }
                    }
                }
            }
            _ => return Err(ToolError::new(format!("unknown config.Adapter field {k}"))),
        }
    }
    let mut srv = ServerConfig::default();
    for (k, v) in fields_of(&server, "config.Server")? {
        match k.as_str() {
            "ExternalUrl" => srv.external_url = string(v, &k)?,
            "DataCenter" => srv.datacenter = string(v, &k)?,
            "GvlID" => {
                let Val::Int(i) = v else {
                    return Err(ToolError::new(format!("unresolved GvlID {}", v.describe())));
                };
                srv.gvl_id = i;
            }
            _ => return Err(ToolError::new(format!("unknown config.Server field {k}"))),
        }
    }
    Ok(Construction {
        construct: Construct::Builder,
        bidder_name: Some(bidder_name),
        adapter: Some(cfg),
        server: Some(srv),
        direct: None,
        built_by,
    })
}

fn fields_of(v: &Val, want: &str) -> Result<Vec<(String, Val)>> {
    let mismatch = || ToolError::new(format!("expected a {want} literal, found {}", v.describe()));
    let Val::Composite { ty, fields } = v else {
        return Err(mismatch());
    };
    if ty != want {
        return Err(mismatch());
    }
    if let Some((k, _)) = fields.iter().find(|(k, _)| k.starts_with('#')) {
        return Err(ToolError::new(format!(
            "{want} literal has a positional element {k}"
        )));
    }
    Ok(fields.clone())
}

fn string(v: Val, what: &str) -> Result<String> {
    let Val::Str(s) = v else {
        return Err(ToolError::new(format!(
            "unresolved {what} {}",
            v.describe()
        )));
    };
    Ok(s)
}

fn clean_rel(dir: &str) -> Result<String> {
    let parts: Vec<&str> = dir
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if dir.starts_with('/') || parts.is_empty() || parts.contains(&"..") {
        return Err(ToolError::new(format!(
            "fixture directory {dir:?} is not a plain relative path"
        )));
    }
    Ok(parts.join("/"))
}

/// Error checks and assertions that do not change the bidder.
///
/// A `return` is never benign: frames stop before their single top-level `return`, so a
/// `return` among the statements before the call means the call is unreachable.
fn is_benign(file: &File, kind: &StmtKind, range: Range<usize>) -> bool {
    match kind {
        StmtKind::Expr(Expr::Call { func, .. }) => matches!(
            render(func).as_str(),
            "assert.NoError"
                | "require.NoError"
                | "assert.Nil"
                | "require.Nil"
                | "assert.NotNil"
                | "require.NotNil"
                | "t.Helper"
        ),
        StmtKind::If => is_err_check(file, range),
        StmtKind::Return(_) | StmtKind::Bind { .. } | StmtKind::Expr(_) | StmtKind::Other => false,
    }
}

/// Whether statement `si` of `func` only binds locals to values without side effects, so when
/// evaluation does not read it, it cannot reach the bidder: `x := <pure>`, `var x = <pure>`, or
/// `x = <pure>` where `x` is a parameter or was declared by an earlier top-level `:=` or `var`
/// (a later declaration or reassignment replaces such a value, as in `x := "a"; x = "b"`).
/// At the top level of a function, `:=` and `var` never assign package variables. A name whose
/// address the function takes (`&x`) is excluded: a pointer would see the new value while
/// evaluation, which copies values, would not.
fn binds_unread_local(file: &File, func: &Func, si: usize) -> bool {
    let Some(StmtKind::Bind {
        names,
        values,
        define,
    }) = func.body.get(si).map(|s| &s.kind)
    else {
        return false;
    };
    let declared = |name: &String| {
        *define
            || func.params.contains(name)
            || func.body.iter().take(si).any(|s| {
                matches!(&s.kind, StmtKind::Bind { names: earlier, define: true, .. }
                    if earlier.contains(name))
            })
    };
    let addressed = |name: &String| {
        func.body.iter().any(|s| {
            file.toks(s.tokens.clone())
                .windows(2)
                .any(|w| matches!(w, [amp, id] if amp.is("&") && id.is_ident() && id.text == *name))
        })
    };
    values.iter().all(is_pure)
        && names
            .iter()
            .all(|n| n == "_" || (declared(n) && !addressed(n)))
}

/// Whether evaluating `e` has no effect besides its value: literals, names, selectors, `+`,
/// `&`, composite literals of such, and `Builder` calls with such arguments (adapter builders
/// only construct; a `Builder` error that would stop the test fails the Go tree's own tests).
fn is_pure(e: &Expr) -> bool {
    match e {
        Expr::Str(_) | Expr::Int(_) | Expr::Ident(_) => true,
        Expr::Selector(x, _) => is_pure(x),
        Expr::Unary { op, expr } => op == "&" && is_pure(expr),
        Expr::Binary { op, lhs, rhs } => op == "+" && is_pure(lhs) && is_pure(rhs),
        Expr::Composite { elems, .. } => elems
            .iter()
            .all(|el| el.key.as_ref().is_none_or(is_pure) && is_pure(&el.value)),
        Expr::Call { func, args } => is_builder(&render(func)) && args.iter().all(is_pure),
        Expr::Other(_) => false,
    }
}

/// `if <ident> != nil { t.X(...); ... }` (no else) where every statement of the block calls a
/// method of `t`.
fn is_err_check(file: &File, range: Range<usize>) -> bool {
    let toks = file.toks(range.clone());
    let head_ok = matches!(toks, [i, x, ne, nil, open, ..]
        if i.is("if") && x.is_ident() && ne.is("!=") && nil.is("nil") && open.is("{"));
    if !head_ok {
        return false;
    }
    let Some((stmts, close)) = file.block(range.start + 4) else {
        return false;
    };
    close + 1 == range.end && stmts.iter().all(|s| calls_method_of_t(&s.kind))
}

fn calls_method_of_t(kind: &StmtKind) -> bool {
    let StmtKind::Expr(Expr::Call { func, .. }) = kind else {
        return false;
    };
    render(func).starts_with("t.")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Construct, Package, Run, parse_go_file};
    use crate::{
        error::{Result, ToolError},
        goparse::File,
        testutil::{check, check_eq},
    };

    const SRC: &str = r#"package demo

const testsDir = "demotest"

func TestPlain(t *testing.T) {
	bidder, buildErr := Builder(openrtb_ext.BidderDemo, config.Adapter{
		Endpoint: "http://" + host + "/x"},
		config.Server{ExternalUrl: "http://hosturl.com", GvlID: 1, DataCenter: "2"})
	if buildErr != nil {
		t.Fatalf("Builder returned unexpected error %v", buildErr)
	}
	adapterstest.RunJSONBidderTest(t, testsDir, bidder)
}

func TestIfInit(t *testing.T) {
	if bidder, err := Builder(openrtb_ext.BidderDemo,
		config.Adapter{}, config.Server{}); err == nil {
		adapterstest.RunJSONBidderTest(t, "other", bidder)
	} else {
		t.Fatalf("x")
	}
}

func TestSetup(t *testing.T) {
	bidder, _ := Builder(openrtb_ext.BidderDemo, config.Adapter{}, config.Server{})
	setFake(bidder)
	adapterstest.RunJSONBidderTest(t, testsDir, bidder)
}

func TestElse(t *testing.T) {
	if false {
	} else {
		adapterstest.RunJSONBidderTest(t, testsDir, nil)
	}
}

var host = "h"
"#;

    #[test]
    fn follows_plain_and_if_init_calls() -> Result<()> {
        let pkg = Package {
            dir: "demo".to_owned(),
            files: vec![File::parse("adapters/demo/demo_test.go", SRC.to_owned())?],
        };
        let consts = BTreeMap::from([("BidderDemo".to_owned(), "demo".to_owned())]);
        let (mut runs, mut errors) = (Vec::new(), Vec::new());
        pkg.runs(&consts, &mut runs, &mut errors);
        let run = |test: &str| {
            runs.iter()
                .find(|r| r.go_test == test)
                .ok_or_else(|| ToolError::new(format!("no run {test}")))
        };

        let plain = run("TestPlain")?;
        check_eq(&plain.construct, &Construct::Builder, "construct")?;
        check_eq(&plain.bidder_name.as_deref(), &Some("demo"), "bidder name")?;
        check_eq(
            plain.fixture_root.as_str(),
            "adapters/demo/demotest",
            "root",
        )?;
        check_eq(
            &plain.adapter.as_ref().map(|a| a.endpoint.as_str()),
            &Some("http://h/x"),
            "endpoint",
        )?;
        check_eq(&plain.server.as_ref().map(|s| s.gvl_id), &Some(1), "gvl id")?;
        check(plain.setup.is_empty(), "error checks are not setup")?;

        let if_init = run("TestIfInit")?;
        check_eq(
            if_init.fixture_root.as_str(),
            "adapters/demo/other",
            "if-init root",
        )?;
        check(
            if_init.setup.is_empty(),
            "the if/else around the call is not setup",
        )?;

        check_eq(
            run("TestSetup")?.setup.as_slice(),
            &["setFake(bidder)".to_owned()][..],
            "setup detected",
        )?;

        check_eq(
            &errors.len(),
            &1,
            "a call in an else branch is not followed",
        )?;
        check(
            errors.first().is_some_and(|e| e.contains("TestElse")),
            "the error names the test",
        )
    }

    /// Evaluates one test file of package `demo`.
    fn evaluate(name: &str, body: &str) -> Result<(Vec<Run>, Vec<String>)> {
        let src = format!("package demo\n\n{body}");
        let pkg = Package {
            dir: "demo".to_owned(),
            files: vec![parse_go_file(&format!("adapters/demo/{name}"), src)?],
        };
        let consts = BTreeMap::from([("BidderDemo".to_owned(), "demo".to_owned())]);
        let (mut runs, mut errors) = (Vec::new(), Vec::new());
        pkg.runs(&consts, &mut runs, &mut errors);
        Ok((runs, errors))
    }

    const BUILD: &str = "bidder, _ := Builder(openrtb_ext.BidderDemo, \
                         config.Adapter{Endpoint: ep}, config.Server{})";
    const RUN: &str = "adapterstest.RunJSONBidderTest(t, \"demotest\", bidder)";

    // Each source is a pattern under which `go test` makes no run, a different run, or a run the
    // source alone does not determine; the generator must refuse each one.
    #[test]
    fn refuses_calls_it_cannot_follow() -> Result<()> {
        let cases = [
            (
                "if-init checking err != nil",
                format!(
                    "func TestA(t *testing.T) {{\n\tep := \"a\"\n\tif bidder, err := \
                     Builder(openrtb_ext.BidderDemo, config.Adapter{{Endpoint: ep}}, \
                     config.Server{{}}); err != nil {{\n\t\t{RUN}\n\t}}\n}}\n"
                ),
                "demo_test.go:TestA: RunJSONBidderTest is called in a form",
            ),
            (
                "if-init with another condition",
                format!(
                    "func TestA(t *testing.T) {{\n\tep := \"a\"\n\tif n := len(ep); n == 2 \
                     {{\n\t\t{BUILD}\n\t\t{RUN}\n\t}}\n}}\n"
                ),
                "demo_test.go:TestA: RunJSONBidderTest is called in a form",
            ),
            (
                "method",
                format!(
                    "type suite struct{{}}\n\nfunc (suite) run(t *testing.T) {{\n\tep := \"a\"\n\t\
                     {BUILD}\n\t{RUN}\n}}\n"
                ),
                "demo_test.go: RunJSONBidderTest is referenced 1 time(s) outside",
            ),
            (
                "package-level function literal",
                format!("var run = func(t *testing.T) {{\n\tep := \"a\"\n\t{BUILD}\n\t{RUN}\n}}\n"),
                "demo_test.go: RunJSONBidderTest is referenced 1 time(s) outside",
            ),
            (
                "helper that is not a test",
                format!("func run(t *testing.T) {{\n\tep := \"a\"\n\t{BUILD}\n\t{RUN}\n}}\n"),
                "demo_test.go:run: RunJSONBidderTest is called from a function that is not",
            ),
            (
                "Test followed by a lower-case letter",
                format!("func Testlower(t *testing.T) {{\n\tep := \"a\"\n\t{BUILD}\n\t{RUN}\n}}\n"),
                "demo_test.go:Testlower: RunJSONBidderTest is called from a function that is not",
            ),
            (
                "package variable assigned by init",
                format!(
                    "var ep = \"a\"\n\nfunc init() {{\n\tep = \"b\"\n}}\n\nfunc TestA(t \
                     *testing.T) {{\n\t{BUILD}\n\t{RUN}\n}}\n"
                ),
                "demo_test.go:TestA: the construction reads package variable(s) {\"ep\"}",
            ),
            (
                "package variable assigned by another test",
                format!(
                    "var ep = \"a\"\n\nfunc TestA(t *testing.T) {{\n\tep = \"b\"\n}}\n\nfunc \
                     TestB(t *testing.T) {{\n\t{BUILD}\n\t{RUN}\n}}\n"
                ),
                "demo_test.go:TestB: the construction reads package variable(s) {\"ep\"}",
            ),
            (
                "two calls",
                format!(
                    "func TestA(t *testing.T) {{\n\tep := \"a\"\n\t{BUILD}\n\t{RUN}\n\t{RUN}\n}}\n"
                ),
                "demo_test.go:TestA: more than one RunJSONBidderTest call",
            ),
        ];
        for (what, src, want) in &cases {
            // Any error fails generation, so the runs recorded next to it never reach a manifest.
            let (runs, errors) = evaluate("demo_test.go", src)?;
            check(
                errors
                    .iter()
                    .any(|e| e.starts_with(&format!("adapters/demo/{want}"))),
                &format!(
                    "{what}: refused with {want:?}; got errors {errors:?}, runs {}",
                    runs.len()
                ),
            )?;
        }
        Ok(())
    }

    // Statements before the call that change what `go test` runs are setup, never ignored.
    #[test]
    fn reports_statements_that_change_the_run() -> Result<()> {
        let cases = [
            (
                "return",
                format!("\tep := \"a\"\n\t{BUILD}\n\treturn\n"),
                "return",
            ),
            (
                "t.Skip",
                format!("\tep := \"a\"\n\t{BUILD}\n\tt.Skip()\n"),
                "t.Skip()",
            ),
            (
                "pointer to a reassigned local",
                "\tep := \"a\"\n\td := directAdapter{Endpoint: ep}\n\tbidder := &d\n\td = \
                 directAdapter{Endpoint: \"b\"}\n"
                    .to_owned(),
                "d = directAdapter{Endpoint: \"b\"}",
            ),
            (
                "pointer to a redeclared local",
                "\tep := \"a\"\n\td := directAdapter{Endpoint: ep}\n\tbidder := &d\n\td, n := \
                 directAdapter{Endpoint: \"b\"}, 1\n\t_ = n\n"
                    .to_owned(),
                "d, n := directAdapter{Endpoint: \"b\"}, 1",
            ),
            (
                "field assignment",
                "\tcfg := config.Adapter{Endpoint: \"a\"}\n\tcfg.Endpoint = \"b\"\n\tbidder, _ := \
                 Builder(openrtb_ext.BidderDemo, cfg, config.Server{})\n"
                    .to_owned(),
                "cfg.Endpoint = \"b\"",
            ),
            (
                "assignment to a package variable",
                format!("\tep := \"a\"\n\tshared = ep\n\t{BUILD}\n"),
                "shared = ep",
            ),
        ];
        for (what, pre, want) in &cases {
            let src =
                format!("var shared = \"\"\n\nfunc TestA(t *testing.T) {{\n{pre}\t{RUN}\n}}\n");
            let (runs, errors) = evaluate("demo_test.go", &src)?;
            let setup: Vec<&[String]> = runs.iter().map(|r| r.setup.as_slice()).collect();
            check(
                errors.is_empty() && setup == [&[(*want).to_owned()][..]],
                &format!("{what}: setup {setup:?}, errors {errors:?}"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn ignores_unread_local_rebinds() -> Result<()> {
        let src = "func TestA(t *testing.T) {\n\
            \tep := \"a\"\n\
            \tep = ep + \"b\"\n\
            \tbidder, err := Builder(openrtb_ext.BidderDemo, config.Adapter{Endpoint: \"x\"},\n\
            \t\tconfig.Server{})\n\
            \tbidder, err = Builder(openrtb_ext.BidderDemo, config.Adapter{Endpoint: ep},\n\
            \t\tconfig.Server{})\n\
            \tif err != nil {\n\
            \t\tt.Fatalf(\"x\")\n\
            \t}\n\
            \t_ = err\n\
            \tadapterstest.RunJSONBidderTest(t, \"demotest\", bidder)\n\
            }\n";
        let (runs, errors) = evaluate("demo_test.go", src)?;
        check(errors.is_empty(), &format!("errors {errors:?}"))?;
        let [run] = runs.as_slice() else {
            return Err(ToolError::new(format!("{} runs", runs.len())));
        };
        check(run.setup.is_empty(), &format!("setup {:?}", run.setup))?;
        check_eq(
            &run.adapter.as_ref().map(|a| a.endpoint.as_str()),
            &Some("ab"),
            "the last binding is the one in effect",
        )
    }

    #[test]
    fn refuses_build_constrained_files() -> Result<()> {
        let body = format!("func TestA(t *testing.T) {{\n\tep := \"a\"\n\t{BUILD}\n\t{RUN}\n}}\n");
        for name in [
            "demo_linux_test.go",
            "demo_arm64_test.go",
            "demo_windows_amd64_test.go",
        ] {
            check(
                evaluate(name, &body).is_err(),
                &format!("{name} is refused"),
            )?;
        }
        for header in ["//go:build ignore\n\n", "// +build linux\n\n"] {
            let src = format!("{header}package demo\n\n{body}");
            check(
                parse_go_file("adapters/demo/demo_test.go", src).is_err(),
                &format!("{header:?} is refused"),
            )?;
        }
        // Not constraints: an unknown suffix, a `_test` suffix alone, a comment after `package`.
        for name in ["demo_linuxish_test.go", "linux_test.go", "demo_test.go"] {
            check(evaluate(name, &body).is_ok(), &format!("{name} is read"))?;
        }
        let late = format!("package demo\n\n//go:build ignore\n\n{body}");
        check(
            parse_go_file("adapters/demo/demo_test.go", late).is_ok(),
            "a //go:build line after the package clause is a plain comment",
        )
    }
}

/// Replays `testdata/manifest_synth.json` (generated by `testdata/go/manifestsynth`, which runs
/// the generated tests with `go test`): the evaluator reproduces Go exactly where it claims to,
/// and refuses loudly everywhere else.
#[cfg(test)]
mod synth {
    use std::{collections::BTreeMap, fs, path::Path};

    use serde::Deserialize;

    use super::{Construct, Package, Run, parse_go_file};
    use crate::{
        error::{Result, ToolError},
        testutil::check,
    };

    #[derive(Deserialize)]
    struct Golden {
        package: String,
        consts: BTreeMap<String, String>,
        files: Vec<SourceFile>,
        cases: Vec<Case>,
        observations: Vec<Observation>,
    }

    #[derive(Deserialize)]
    struct SourceFile {
        name: String,
        src: String,
    }

    #[derive(Deserialize)]
    struct Case {
        test: String,
        expect: String,
        blame: String,
        note: String,
    }

    #[derive(Deserialize, Debug, PartialEq, Eq)]
    struct Observation {
        test: String,
        root: String,
        bidder_name: Option<String>,
        adapter: Option<BTreeMap<String, String>>,
        server: Option<ObservedServer>,
        direct: Option<BTreeMap<String, String>>,
    }

    #[derive(Deserialize, Debug, PartialEq, Eq)]
    struct ObservedServer {
        external_url: String,
        gvl_id: i64,
        datacenter: String,
    }

    /// What the Go fake would have recorded for `run`.
    fn observation(run: &Run, test: &str) -> Observation {
        let direct = run.direct.as_ref().map(|d| {
            ["Endpoint", "Extra"]
                .into_iter()
                .map(|k| (k.to_owned(), d.fields.get(k).cloned().unwrap_or_default()))
                .collect::<BTreeMap<_, _>>()
        });
        Observation {
            test: test.to_owned(),
            root: run.fixture_root.clone(),
            bidder_name: run.bidder_name.clone(),
            adapter: run.adapter.as_ref().map(|a| {
                BTreeMap::from([
                    ("app_secret".to_owned(), a.app_secret.clone()),
                    ("endpoint".to_owned(), a.endpoint.clone()),
                    ("extra_info".to_owned(), a.extra_info.clone()),
                    ("platform_id".to_owned(), a.platform_id.clone()),
                    ("xapi_password".to_owned(), a.xapi.password.clone()),
                    ("xapi_tracker".to_owned(), a.xapi.tracker.clone()),
                    ("xapi_username".to_owned(), a.xapi.username.clone()),
                ])
            }),
            server: run.server.as_ref().map(|s| ObservedServer {
                external_url: s.external_url.clone(),
                gvl_id: s.gvl_id,
                datacenter: s.datacenter.clone(),
            }),
            direct,
        }
    }

    #[test]
    fn evaluator_matches_go_or_refuses() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("manifest_synth.json");
        let golden: Golden = serde_json::from_slice(&fs::read(&path)?)?;
        let mut errors = Vec::new();
        let mut files = Vec::new();
        for f in &golden.files {
            let rel = format!("adapters/{}/{}", golden.package, f.name);
            match parse_go_file(&rel, f.src.clone()) {
                Ok(file) => files.push(file),
                Err(e) => errors.push(e.to_string()),
            }
        }
        let pkg = Package {
            dir: golden.package.clone(),
            files,
        };
        let mut runs = Vec::new();
        pkg.runs(&golden.consts, &mut runs, &mut errors);

        let (mut matched, mut refused) = (0, 0);
        let mut failures = Vec::new();
        for case in &golden.cases {
            // Subtests report `Test/sub`; they belong to the top-level test.
            let observed: Vec<&Observation> = golden
                .observations
                .iter()
                .filter(|o| o.test.split('/').next() == Some(case.test.as_str()))
                .collect();
            let mine: Vec<&Run> = runs.iter().filter(|r| r.go_test == case.test).collect();
            let blamed = errors
                .iter()
                .any(|e| e.starts_with(&format!("{}:", case.blame)));
            let loud = blamed
                || mine.iter().any(|r| {
                    !r.setup.is_empty()
                        || r.direct
                            .as_ref()
                            .is_some_and(|d| !d.opaque_fields.is_empty())
                });
            let evaluated: Vec<Observation> =
                mine.iter().map(|r| observation(r, &case.test)).collect();
            let describe = || {
                format!(
                    "{} ({}): go observed {observed:?}; rust evaluated {evaluated:?}; blamed \
                     {blamed}",
                    case.test, case.note,
                )
            };
            match case.expect.as_str() {
                "match" => {
                    let ok = !loud
                        && observed.len() == 1
                        && evaluated.len() == 1
                        && observed.iter().zip(&evaluated).all(|(o, e)| *o == e);
                    if ok {
                        matched += 1;
                    } else {
                        failures.push(format!("match: {}", describe()));
                    }
                }
                "loud" => {
                    if loud {
                        refused += 1;
                    } else {
                        failures.push(format!("not refused: {}", describe()));
                    }
                }
                "none" => {
                    if !observed.is_empty() || !mine.is_empty() {
                        failures.push(format!("expected no run: {}", describe()));
                    }
                }
                other => failures.push(format!("{}: unknown expectation {other}", case.test)),
            }
        }
        // Every clean run the evaluator produced is a case that matched.
        for run in &runs {
            if !golden.cases.iter().any(|c| c.test == run.go_test) {
                failures.push(format!("{}: a run for no case", run.go_test));
            }
        }
        check(
            runs.iter()
                .all(|r| r.construct == Construct::Builder || r.direct.is_some()),
            "direct runs carry their literal",
        )?;
        if !failures.is_empty() {
            return Err(ToolError::new(format!(
                "{} of {} synthetic cases diverge from Go:\n  {}\nerrors:\n  {}",
                failures.len(),
                golden.cases.len(),
                failures.join("\n  "),
                errors.join("\n  ")
            )));
        }
        check(
            matched >= 200 && refused >= 20,
            &format!("coverage: {matched} matched, {refused} refused"),
        )
    }
}
