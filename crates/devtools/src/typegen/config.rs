//! `typegen.json`: which Go packages a crate generates, where the Rust goes, and the hand-written
//! exceptions.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    error::{Result, ToolError},
    repo::Repo,
};

/// The file name typegen looks for in each crate directory.
pub const CONFIG_FILE: &str = "typegen.json";

/// One crate's `typegen.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The Rust crate name as code refers to it (`pbs_openrtb`).
    #[serde(rename = "crate")]
    pub crate_name: String,
    /// The Go packages the crate generates.
    pub packages: Vec<PackageConfig>,
    /// Where to write the Rust visitor over every generated type and constant (for the crate's
    /// tests), relative to the crate directory.
    #[serde(default)]
    pub rust_index: Option<String>,
    /// Where to write a Go file listing every generated type (for the crate's Go golden
    /// generators).
    #[serde(default)]
    pub go_index: Option<GoIndex>,
    /// Go types defined outside any generated package, mapped to Rust types (`"time.Duration"`
    /// style keys: import path, a dot, the type name).
    #[serde(default)]
    pub extern_types: BTreeMap<String, ExternType>,
    /// The crate directory (set when the file is loaded).
    #[serde(skip)]
    pub dir: PathBuf,
    /// The config file's workspace-relative path (set when the file is loaded).
    #[serde(skip)]
    pub source: String,
}

/// The Go index file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoIndex {
    /// The file, relative to the crate directory.
    pub out: String,
    /// Its Go package name.
    pub package: String,
}

/// How generated Rust names a struct field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldNames {
    /// From the JSON key (`bidfloorcur`, `us_privacy`, `placementId` → `placement_id`), falling
    /// back to the Go name when the field has none.
    Json,
    /// From the Go field name (`BidFloorCur` → `bid_floor_cur`).
    #[default]
    Go,
}

/// The Rust type a Go `string` becomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum StringType {
    /// `String` (invalid UTF-8 from jsoniter input is replaced by U+FFFD).
    #[default]
    String,
    /// `pbs_gojson::GoString` (Go's exact bytes).
    GoString,
}

/// One generated Go package.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageConfig {
    /// The Go import path.
    pub go: String,
    /// The Rust module path inside the crate (`openrtb2`, `native1::request`).
    pub rust: String,
    /// The generated file, relative to the crate directory.
    pub out: String,
    /// The field naming policy.
    #[serde(default)]
    pub field_names: FieldNames,
    /// The Rust type for Go strings.
    #[serde(default)]
    pub strings: StringType,
    /// Hand-written exceptions, keyed by `Type`, `Type.Field` or a constant name.
    #[serde(default)]
    pub overrides: BTreeMap<String, Override>,
    /// The hand-written Rust port of every exported Go function (`Name`) and method
    /// (`Type.Method`) of the package. Generation fails when the Go package gains or loses one.
    #[serde(default)]
    pub functions: BTreeMap<String, FunctionPort>,
}

/// Where the hand-written port of a Go function or method is, and its signature.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionPort {
    /// The Rust path inside the package's module (`int8_ptr`, `NoBidReason::ptr`).
    pub rust: String,
    /// Its type as a function pointer, with types qualified by their module as the Rust index
    /// sees them (`fn(openrtb3::NoBidReason) -> Option<openrtb3::NoBidReason>`).
    pub signature: String,
}

/// A hand-written exception for one type, field or constant.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// Why the exception exists (required, so the file explains itself).
    pub reason: String,
    /// Do not generate this type, field or constant (a type is then written by hand in the
    /// same module, under the same Rust name).
    #[serde(default)]
    pub skip: bool,
    /// The Rust name to use instead of the derived one.
    #[serde(default)]
    pub rust_name: Option<String>,
    /// A field's Rust type, written verbatim.
    #[serde(default)]
    pub rust_type: Option<String>,
    /// `use` paths the `rust_type` needs.
    #[serde(default)]
    pub uses: Vec<String>,
    /// Which of `Copy` and `Eq` the `rust_type`, or the hand-written type of a skipped type,
    /// implements (they decide the derives of the structs holding it).
    #[serde(default)]
    pub implements: Vec<Derive>,
    /// The Go type an untyped constant is given (a type of the same package, or a basic type).
    #[serde(default)]
    pub go_type: Option<String>,
    /// Hold a pointer-to-struct field as `Option<T>` instead of `Option<Box<T>>`.
    #[serde(default)]
    pub inline: bool,
}

impl Override {
    /// Whether the overriding Rust type is `Copy`.
    #[must_use]
    pub fn is_copy(&self) -> bool {
        self.implements.contains(&Derive::Copy)
    }

    /// Whether the overriding Rust type is `Eq`.
    #[must_use]
    pub fn is_eq(&self) -> bool {
        self.implements.contains(&Derive::Eq)
    }
}

/// A trait that decides the derives of the structs holding a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Derive {
    /// `Copy`.
    Copy,
    /// `Eq`.
    Eq,
}

/// A Go type mapped to an existing Rust type.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternType {
    /// The Rust type as generated code spells it (`RawJson`, `jsonutil::StringInt`).
    pub rust: String,
    /// The `use` path that brings it into scope.
    #[serde(rename = "use")]
    pub use_path: String,
    /// Whether the Rust type is `Copy`.
    #[serde(default)]
    pub copy: bool,
    /// Whether the Rust type is `Eq`.
    #[serde(default)]
    pub eq: bool,
    /// Whether the Rust type is a Go struct (held behind `Box` when pointed to).
    #[serde(default)]
    pub is_struct: bool,
}

impl Config {
    /// The package config for a Go import path.
    #[must_use]
    pub fn package(&self, go: &str) -> Option<&PackageConfig> {
        self.packages.iter().find(|p| p.go == go)
    }
}

/// Every `crates/*/typegen.json`, sorted by crate directory.
///
/// # Errors
///
/// Fails when a file cannot be read or parsed, or a Go package is configured twice.
pub fn discover(repo: &Repo) -> Result<Vec<Config>> {
    let crates = repo.path("crates");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&crates)
        .map_err(|e| ToolError::io(&crates, &e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.join(CONFIG_FILE).is_file())
        .collect();
    dirs.sort();
    let mut configs = Vec::new();
    for dir in dirs {
        configs.push(load(repo, &dir)?);
    }
    let mut seen = BTreeMap::new();
    for config in &configs {
        for p in &config.packages {
            if let Some(other) = seen.insert(p.go.clone(), config.source.clone()) {
                return Err(ToolError::new(format!(
                    "Go package {} is generated by both {other} and {}",
                    p.go, config.source
                )));
            }
        }
    }
    Ok(configs)
}

/// Loads `dir/typegen.json`.
///
/// # Errors
///
/// Fails when the file cannot be read or parsed.
pub fn load(repo: &Repo, dir: &Path) -> Result<Config> {
    let path = dir.join(CONFIG_FILE);
    let bytes = fs::read(&path).map_err(|e| ToolError::io(&path, &e))?;
    let mut config: Config = serde_json::from_slice(&bytes)
        .map_err(|e| ToolError::new(format!("{}: {e}", path.display())))?;
    config.dir = dir.to_path_buf();
    config.source = path
        .strip_prefix(repo.workspace())
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    for p in &config.packages {
        for (key, o) in &p.overrides {
            if o.reason.trim().is_empty() {
                return Err(ToolError::new(format!(
                    "{}: override {} of {} needs a reason",
                    config.source, key, p.go
                )));
            }
        }
    }
    Ok(config)
}
