//! `pbs-devtools typegen`: Rust types for Go packages, generated from what go/types says about
//! them.
//!
//! Every `crates/<crate>/typegen.json` ([`config`]) names Go packages and where their Rust
//! modules go. Generation runs the Go describer `crates/devtools/go/typegen` once over all
//! configured packages, as the Go root's build resolves them (it type-checks them from source
//! and records, in source order, every type
//! with its fields, tags and doc comments, every constant with its exact value, and every
//! function and method; see [`model`]), then writes for each package one Rust file:
//!
//! - a struct per Go struct and a single-field tuple struct per Go named type, with
//!   `#[derive(GoJson)]`, `#[gojson(go_type = "pkg.Name")]` and, per field, the Go name and the
//!   `json` tag verbatim (`#[gojson(go = "ID", json = "id,omitempty")]`), so pbs-gojson encodes and
//!   decodes them as Go does;
//! - a `pub const` per exported constant (`PositionUnknown` → `POSITION_UNKNOWN`), and a `pub use`
//!   per type alias;
//! - Go doc comments as rustdoc ([`docs`]), and `#[doc(alias = "GoName")]` on every type and
//!   constant whose Rust name differs from its Go name, so that rustdoc search and grep find it by
//!   the Go name.
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
//! A config with `"serde": true` also gets serde code next to the `GoJson` derive ([`serde_code`]):
//! `Serialize` and `Deserialize` with a container `#[serde(default)]`, per field a `rename` to the
//! JSON key, `skip_serializing_if` for `omitempty` (`Option::is_none` for pointers,
//! `is_nil_or_empty` for slices and maps, `is_zero` for numbers, bools and strings, never for
//! structs), `deserialize_with = "null_default"` on every value field, `#[serde(transparent)]` on
//! named types, and a pbs-jsonutil `MergeClone` impl per type that merges each member into its
//! field with Go's context (`Type.Field`). A struct that embeds another writes it
//! `#[serde(flatten)]` and reads every member, promoted ones included, through a private mirror
//! struct (`#[serde(from = "...Fields")]`). The config's `serde_bridge` names the module that
//! carries what pbs-jsonutil cannot read, write or merge itself (`RawJson`, the merge of Go `int`
//! and `uint`, and the `extern_types` that name a bridge); a field override's `keep_null` keeps
//! null in a `json.RawMessage` of a type Prebid decodes with `encoding/json`. Generation refuses
//! what serde cannot hold as Go does: a struct without fields, two fields with one JSON key, the
//! `,string` option, `GoString` strings, a bridged type inside a pointer, slice or map or under a
//! named type, a named type of a slice, map or struct, and an embedded struct that is a pointer,
//! of another package or embeds one itself.
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
pub mod serde_code;
#[cfg(test)]
mod serde_tests;

use std::{collections::BTreeMap, ffi::OsString, fs, path::Path};

use self::{config::Config, model::Description, registry::Registry};
use crate::{
    error::{Result, ToolError},
    repo::{self, GO_TOOLS, Mode, Outcome, Repo, TempDir},
};

/// The Go describer, relative to the [`GO_TOOLS`] module.
pub const DESCRIBER: &str = "./typegen";

/// Generates every configured crate's files, writing them or (in check mode) comparing them with
/// the committed copies. Returns each file's workspace-relative path and outcome.
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

/// The crate directory, relative to the workspace root.
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
    repo.go(GO_TOOLS, &args)?;
    let bytes = fs::read(&out).map_err(|e| ToolError::io(&out, &e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ToolError::new(format!("parsing the Go description: {e}")))
}
