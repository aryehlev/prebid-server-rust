//! `pbs-devtools typegen`: Rust types for Go packages, generated from what go/types says about
//! them.
//!
//! Every `rust/crates/<crate>/typegen.json` ([`config`]) names Go packages and where their Rust
//! modules go. Generation runs the Go describer `rust/crates/devtools/go/typegen` once over all
//! configured packages (it type-checks them from source and records, in source order, every type
//! with its fields, tags and doc comments, every constant with its exact value, and every
//! function and method; see [`model`]), then writes for each package one Rust file:
//!
//! - a struct per Go struct and a single-field tuple struct per Go named type, with
//!   `#[derive(GoJson)]`, `#[gojson(go_type = "pkg.Name")]` and, per field, the Go name and the
//!   `json` tag verbatim (`#[gojson(go = "ID", json = "id,omitempty")]`), so pbs-gojson encodes and
//!   decodes them as Go does;
//! - a `pub const` per exported constant (`PositionUnknown` → `POSITION_UNKNOWN`), and a `pub use`
//!   per type alias;
//! - Go doc comments as rustdoc ([`docs`]).
//!
//! Types map exactly ([`registry`]): `int8` → `i8`, `int` → `isize` (and so on for every width),
//! `string` → `String` (or `GoString`), pointers to structs → `Option<Box<T>>`, other pointers →
//! `Option<T>`, slices → `Option<Vec<T>>` and maps → `Option<BTreeMap<K, V>>` (nil and empty
//! differ in Go), `json.RawMessage` → `RawJson`, `interface{}` → `serde_json::Value`, types of
//! other configured packages by their module path (`adcom1::DeviceType`, also across crates),
//! and anything else through the config's `extern_types`. Copy and Eq are derived whenever every
//! field allows them. Field names come from the JSON key (`field_names: "json"`) or the Go name.
//!
//! Generation refuses what it cannot represent faithfully rather than guessing: an untyped
//! constant, an interface or generic type, an unexported field, a name collision. The config's
//! `overrides` (keyed `Type`, `Type.Field` or `Const`, each with a `reason`) settle those: skip
//! a type to write it by hand next to the generated file, rename, give a field a Rust type (with
//! the `uses` it needs and which of `Copy` and `Eq` it `implements`), hold a pointer to a struct
//! `inline`, give an untyped constant its type. An override that no longer matches anything
//! fails generation, and so does an exported Go function or method that the config's
//! `functions` does not map to its hand-written port (or a mapping whose Go function is gone).
//!
//! Each crate can also ask for two indexes ([`index`]): a Rust file with the `for_each_type!`,
//! `for_each_struct!` and `for_each_const!` macros, the Go names of the types and constants, and
//! a compile-time signature check of every function port (for its parity tests), and a Go file
//! listing the same types as `reflect.Type`s (for its Go golden generators). `--check`
//! regenerates everything in memory and reports stale files.

pub mod config;
pub mod docs;
pub mod emit;
pub mod index;
pub mod model;
pub mod names;
pub mod registry;

use std::{collections::BTreeMap, env, ffi::OsString, fs, path::Path, process::Command};

use self::{config::Config, model::Description, registry::Registry};
use crate::{
    error::{Result, ToolError},
    repo::{self, Mode, Outcome, Repo, TempDir},
};

/// The Go describer, relative to the repository root.
pub const DESCRIBER: &str = "./rust/crates/devtools/go/typegen";

/// Generates every configured crate's files, writing them or (in check mode) comparing them with
/// the committed copies. Returns each file's repository-relative path and outcome.
///
/// # Errors
///
/// Fails when a config is invalid, the Go describer fails, or a package cannot be generated.
pub fn run(repo: &Repo, mode: Mode) -> Result<Vec<(String, Outcome)>> {
    let configs = config::discover(repo)?;
    let go_paths: Vec<String> = configs
        .iter()
        .flat_map(|c| c.packages.iter().map(|p| p.go.clone()))
        .collect();
    if go_paths.is_empty() {
        return Ok(Vec::new());
    }
    let packages: BTreeMap<String, model::Package> = describe(repo, &go_paths)?
        .packages
        .into_iter()
        .map(|p| (p.path.clone(), p))
        .collect();
    let reg = Registry::build(&configs, &packages)?;
    let mut outcomes = Vec::new();
    for config in &configs {
        for (rel, content) in generate(&reg, config, &packages)? {
            let path = config.dir.join(&rel);
            let outcome = repo::emit(&path, content.as_bytes(), mode)?;
            outcomes.push((format!("{}/{rel}", crate_dir(config)), outcome));
        }
    }
    Ok(outcomes)
}

/// Every file of one crate: `(path relative to the crate directory, content)`.
///
/// # Errors
///
/// Fails when a package cannot be generated.
pub fn generate(
    reg: &Registry,
    config: &Config,
    packages: &BTreeMap<String, model::Package>,
) -> Result<Vec<(String, String)>> {
    let mut generated = Vec::new();
    for pc in &config.packages {
        let pkg = packages
            .get(&pc.go)
            .ok_or_else(|| ToolError::new(format!("package {} was not described", pc.go)))?;
        generated.push(emit::package(reg, config, pc, pkg)?);
    }
    let mut files: Vec<(String, String)> = generated
        .iter()
        .map(|g| (g.out.clone(), g.content.clone()))
        .collect();
    if let Some(out) = &config.rust_index {
        files.push((out.clone(), index::rust(config, &generated)));
    }
    if let Some(gi) = &config.go_index {
        files.push((gi.out.clone(), index::go(config, gi, &generated, packages)?));
    }
    Ok(files)
}

/// The crate directory, relative to the repository root.
fn crate_dir(config: &Config) -> &str {
    config
        .source
        .rsplit_once('/')
        .map_or(config.source.as_str(), |(dir, _)| dir)
}

/// Runs the Go describer over the packages.
///
/// # Errors
///
/// Fails when Go fails or its output cannot be parsed.
pub fn describe(repo: &Repo, go_paths: &[String]) -> Result<Description> {
    let tmp = TempDir::new("pbs-devtools-typegen")?;
    let result = describe_into(repo, go_paths, tmp.path());
    let removed = tmp.remove();
    let desc = result?;
    removed?;
    Ok(desc)
}

fn describe_into(repo: &Repo, go_paths: &[String], tmp: &Path) -> Result<Description> {
    let out = tmp.join("description.json");
    let mut args: Vec<OsString> = vec![
        "run".into(),
        DESCRIBER.into(),
        "-out".into(),
        out.clone().into_os_string(),
    ];
    args.extend(go_paths.iter().map(OsString::from));
    go(repo, &args)?;
    let bytes = fs::read(&out).map_err(|e| ToolError::io(&out, &e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ToolError::new(format!("parsing the Go description: {e}")))
}

/// Runs `go` (or `$GO`) in the repository root without `PBS_*` variables.
fn go(repo: &Repo, args: &[OsString]) -> Result<()> {
    let go = env::var_os("GO").unwrap_or_else(|| "go".into());
    let mut cmd = Command::new(&go);
    let _ = cmd.args(args).current_dir(repo.root());
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
    Err(ToolError::new(format!(
        "go {} failed ({}):\n{stderr}",
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "),
        output.status
    )))
}
