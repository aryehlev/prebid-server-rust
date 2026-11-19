//! Glue (Rule B (c)): the configuration file `config.SetupViper` reads: Viper's search
//! (`findConfigFile` and `searchInPath` over `SupportedExts`, viper.go v1.12.0), which decides
//! which file an operator's deployment loads (ledger: `config.file-search`), and the Go types its
//! readers give values.
//!
//! Viper looks in each search path in turn (`.` then `/etc/config`) for `pbs.<ext>` in the order
//! of `SupportedExts`, skipping directories, and reads the first file found, never merging two.
//! JSON goes through `serde_json`, every number a `float64` as encoding/json gives it (ledger:
//! `config.json-reader`), YAML through `serde-saphyr` with yaml.v3's untyped resolution where it
//! agrees (ledger: `yaml.scalars`), and TOML through the `config` crate's reader (ledger:
//! `config.toml-reader`). The other formats Viper reads fail the load (D-VIPERFMT, ledger:
//! `config.formats`). Every reader's tree then has its keys lower-cased as Viper's
//! `insensitiviseMap` does.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf, absolute},
    str,
};

use config::{FileFormat, FileStoredFormat, Format, Map, Value, ValueKind};
use pbs_gocompat::strconv::quote;
use serde::de::{self, Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value as Json;
use serde_saphyr::{
    DuplicateKeyPolicy, NonFiniteFloatPolicy, Options,
    granit_parser::{Event, Parser, YamlVersion},
};

use super::{
    cast,
    value::GoValue,
    viper::{Tree, insensitivise_map},
};

/// Go: `viper.SupportedExts`, in the order Viper searches them.
pub const SUPPORTED_EXTS: &[&str] = &[
    "json",
    "toml",
    "yaml",
    "yml",
    "properties",
    "props",
    "prop",
    "hcl",
    "tfvars",
    "dotenv",
    "env",
    "ini",
];

/// The extensions this port reads (D-VIPERFMT).
const READ_EXTS: &[&str] = &["json", "toml", "yaml", "yml"];

/// What the search found.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Found {
    /// No file: Go's `ConfigFileNotFoundError` text.
    Missing(String),
    /// A file in a format this port does not read.
    Unsupported {
        /// The file.
        path: PathBuf,
        /// Its extension.
        ext: &'static str,
    },
    /// A file this port reads, and what reading it gave: the tree, or the error Viper's
    /// `ReadInConfig` returns (which `config.SetupViper` ignores).
    Read {
        /// The file.
        path: PathBuf,
        /// The index of the search path it is in.
        dir: usize,
        /// The tree or the read error.
        result: Result<Tree, String>,
    },
}

/// Go: `findConfigFile` and `ReadInConfig` for the configuration name `name`.
pub(super) fn search(name: &str, search_paths: &[PathBuf]) -> Found {
    for (dir, base) in search_paths.iter().enumerate() {
        for ext in SUPPORTED_EXTS {
            let path = base.join(format!("{name}.{ext}"));
            // Go: exists() is fs.Stat without error and not a directory.
            if !fs::metadata(&path).is_ok_and(|m| !m.is_dir()) {
                continue;
            }
            if !READ_EXTS.contains(ext) {
                return Found::Unsupported { path, ext };
            }
            let result = fs::read(&path)
                .map_err(|e| format!("open {}: {e}", path.display()))
                .and_then(|bytes| {
                    parse(ext, &bytes).map_err(|e| format!("While parsing config: {e}"))
                });
            return Found::Read { path, dir, result };
        }
    }
    let locations: Vec<String> = search_paths.iter().map(|p| abs_pathify(p)).collect();
    Found::Missing(format!(
        "Config File {} Not Found in {}",
        quote(name),
        quote(&format!("[{}]", locations.join(" ")))
    ))
}

/// Go: viper's `absPathify` of a search path, for the not-found text.
fn abs_pathify(path: &Path) -> String {
    absolute(path).map_or_else(
        |_| path.display().to_string(),
        |p| {
            let text = p.display().to_string();
            text.strip_suffix("/.")
                .map_or_else(|| text.clone(), str::to_owned)
        },
    )
}

/// Reads a file's bytes in the format of `ext` into Viper's tree.
///
/// # Errors
///
/// The reader's error text.
pub(super) fn parse(ext: &str, bytes: &[u8]) -> Result<Tree, String> {
    let mut tree = match ext {
        "json" => parse_json(bytes)?,
        "toml" => parse_toml(bytes)?,
        _ => parse_yaml(bytes)?,
    };
    insensitivise_map(&mut tree);
    Ok(tree)
}

/// Go: `json.Unmarshal(b, &map[string]interface{})`, read with `serde_json`: a document
/// `serde_json` refuses (invalid UTF-8, a lone surrogate escape, nesting 128 levels or more) fails
/// where encoding/json reads it, and the errors are `serde_json`'s (ledger: `config.json-reader`).
fn parse_json(bytes: &[u8]) -> Result<Tree, String> {
    let value: Json = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let kind = match value {
        Json::Null => return Ok(Tree::new()),
        Json::Object(map) => return Ok(map.into_iter().map(|(k, v)| (k, from_json(v))).collect()),
        Json::Array(_) => "array",
        Json::String(_) => "string",
        Json::Number(_) => "number",
        Json::Bool(_) => "bool",
    };
    Err(format!(
        "json: cannot unmarshal {kind} into Go value of type map[string]interface {{}}"
    ))
}

/// An encoding/json `interface{}` value.
fn from_json(value: Json) -> GoValue {
    match value {
        Json::Null => GoValue::Nil,
        Json::Bool(b) => GoValue::Bool(b),
        Json::Number(n) => GoValue::Float64(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => GoValue::String(s),
        Json::Array(items) => GoValue::List(items.into_iter().map(from_json).collect()),
        Json::Object(map) => {
            GoValue::Map(map.into_iter().map(|(k, v)| (k, from_json(v))).collect())
        }
    }
}

/// TOML through the `config` crate's reader: integers are Go's `int64` (go-toml reads them so),
/// date-times strings.
fn parse_toml(bytes: &[u8]) -> Result<Tree, String> {
    let text = str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let map = Format::parse(&FileFormat::Toml, None, text).map_err(|e| e.to_string())?;
    Ok(map.into_iter().map(|(k, v)| (k, from_config(v))).collect())
}

/// A `config` value as go-toml would give it.
fn from_config(value: Value) -> GoValue {
    match value.kind {
        ValueKind::Nil => GoValue::Nil,
        ValueKind::Boolean(b) => GoValue::Bool(b),
        ValueKind::I64(i) => GoValue::Int64(i),
        ValueKind::I128(i) => i64::try_from(i).map_or(GoValue::Nil, GoValue::Int64),
        ValueKind::U64(u) => i64::try_from(u).map_or(GoValue::Uint64(u), GoValue::Int64),
        ValueKind::U128(u) => u64::try_from(u).map_or(GoValue::Nil, GoValue::Uint64),
        ValueKind::Float(f) => GoValue::Float64(f),
        ValueKind::String(s) => GoValue::String(s),
        ValueKind::Table(map) => {
            GoValue::Map(map.into_iter().map(|(k, v)| (k, from_config(v))).collect())
        }
        ValueKind::Array(items) => GoValue::List(items.into_iter().map(from_config).collect()),
    }
}

/// The `serde-saphyr` options nearest yaml.v3's untyped resolution.
///
/// YAML 1.2 core booleans, YAML 1.1 octals (`0777`), duplicate keys an error, and `.inf`/`.nan`
/// floats; errors are one line, as yaml.v3's are, without a snippet of the document.
#[must_use]
pub fn yaml_options() -> Options {
    serde_saphyr::options! {
        duplicate_keys: DuplicateKeyPolicy::Error,
        legacy_octal_numbers: true,
        strict_booleans: true,
        non_finite_float_policy: NonFiniteFloatPolicy::PassThrough,
        with_snippet: false,
    }
}

/// Go: `yaml.Unmarshal(b, &map[string]interface{})`, which reads the first document.
pub(super) fn parse_yaml(bytes: &[u8]) -> Result<Tree, String> {
    let text = str::from_utf8(bytes).map_err(|e| format!("yaml: {e}"))?;
    let Some(document) = first_document(text)? else {
        return Ok(Tree::new());
    };
    let doc: YamlDocument =
        serde_saphyr::from_str_with_options(document, yaml_options()).map_err(|e| e.to_string())?;
    Ok(doc.0)
}

/// The first document of a YAML stream, as yaml.v3's `Unmarshal` reads it; `None` for a stream
/// with none.
///
/// yaml.v3 decodes the first document alone, so a later one is never parsed (an error there, or
/// a later document that is not a mapping, does not fail the read), and its parser takes a
/// `%YAML` directive only for version 1.1 (`yaml_parser_process_directives`, parserc.go: "found
/// incompatible YAML document"). serde-saphyr reads every document, skipping null ones, and takes
/// YAML 1.2, so the loader asks the parser under serde-saphyr where the first document ends and
/// what version it declares. A syntax error before that end is left to serde-saphyr to report.
fn first_document(text: &str) -> Result<Option<&str>, String> {
    for event in Parser::new_from_str(text) {
        let Ok((event, span)) = event else {
            return Ok(Some(text));
        };
        if let Event::DocumentStart(_, Some(version)) = event
            && version != YamlVersion::new(1, 1)
        {
            return Err("yaml: found incompatible YAML document".to_owned());
        }
        if matches!(event, Event::DocumentEnd) {
            return Ok(Some(
                span.end
                    .byte_offset()
                    .and_then(|end| text.get(..end))
                    .unwrap_or(text),
            ));
        }
    }
    Ok(None)
}

/// A YAML document decoded into `map[string]interface{}`: the top-level keys as their text, a
/// null key dropped (yaml.v3 cannot decode null into a `string` key), and an empty or null
/// document an empty map.
#[derive(Debug)]
struct YamlDocument(Tree);

impl<'de> Deserialize<'de> for YamlDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(DocumentVisitor)
    }
}

/// The visitor of [`YamlDocument`].
#[derive(Debug, Clone, Copy)]
struct DocumentVisitor;

impl<'de> Visitor<'de> for DocumentVisitor {
    type Value = YamlDocument;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a mapping")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(YamlDocument(Tree::new()))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(YamlDocument(Tree::new()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut tree = Tree::new();
        while let Some(key) = map.next_key::<Option<String>>()? {
            match key {
                Some(key) => {
                    let value: YamlValue = map.next_value()?;
                    let _previous = tree.insert(key, value.0);
                }
                None => {
                    let _ignored: IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(YamlDocument(tree))
    }
}

/// A YAML node decoded into `interface{}` by yaml.v3's untyped resolution.
#[derive(Debug)]
struct YamlValue(GoValue);

impl<'de> Deserialize<'de> for YamlValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor).map(YamlValue)
    }
}

/// The visitor of [`YamlValue`].
#[derive(Debug, Clone, Copy)]
struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = GoValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any YAML value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(GoValue::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(GoValue::Int(v))
    }

    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Self::Value, E> {
        Ok(i64::try_from(v).map_or_else(
            |_| GoValue::Float64(v.to_string().parse().unwrap_or(0.0)),
            GoValue::Int,
        ))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        // yaml.v3: an int when it fits, else a uint64.
        Ok(i64::try_from(v).map_or(GoValue::Uint64(v), GoValue::Int))
    }

    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Self::Value, E> {
        Ok(u64::try_from(v).map_or_else(
            |_| GoValue::Float64(v.to_string().parse().unwrap_or(0.0)),
            |v| i64::try_from(v).map_or(GoValue::Uint64(v), GoValue::Int),
        ))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(GoValue::Float64(v))
    }

    fn visit_char<E: de::Error>(self, v: char) -> Result<Self::Value, E> {
        Ok(GoValue::String(v.to_string()))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(GoValue::String(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(GoValue::String(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
        Ok(GoValue::String(String::from_utf8_lossy(v).into_owned()))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(GoValue::Nil)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(GoValue::Nil)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<YamlValue>()? {
            items.push(item.0);
        }
        Ok(GoValue::List(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries = Vec::new();
        while let Some(key) = map.next_key::<YamlValue>()? {
            if key.0.is_map() || key.0.is_slice() {
                return Err(de::Error::custom(format!("invalid map key: {}", key.0)));
            }
            let value: YamlValue = map.next_value()?;
            entries.push((key.0, value.0));
        }
        // yaml.v3 decodes a mapping whose keys are all strings into map[string]interface{},
        // any other into map[interface{}]interface{}.
        if entries.iter().all(|(k, _)| matches!(k, GoValue::String(_))) {
            Ok(GoValue::Map(
                entries
                    .into_iter()
                    .map(|(k, v)| (cast::to_string(&k), v))
                    .collect(),
            ))
        } else {
            Ok(GoValue::AnyMap(entries))
        }
    }
}

/// YAML for the `config` crate through `serde-saphyr` with the loader's resolution: what
/// `pbs.yaml` gives Viper, as `config` values.
#[derive(Debug, Clone, Copy, Default)]
pub struct YamlFormat;

impl Format for YamlFormat {
    fn parse(
        &self,
        _uri: Option<&String>,
        text: &str,
    ) -> Result<Map<String, Value>, Box<dyn Error + Send + Sync>> {
        let tree = parse("yaml", text.as_bytes())?;
        Ok(to_config_map(&tree))
    }
}

impl FileStoredFormat for YamlFormat {
    fn file_extensions(&self) -> &'static [&'static str] {
        &["yaml", "yml"]
    }
}

/// JSON for the `config` crate as encoding/json reads it into `map[string]interface{}` (every
/// number a `float64`): what `pbs.json` gives Viper, as `config` values.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonFormat;

impl Format for JsonFormat {
    fn parse(
        &self,
        _uri: Option<&String>,
        text: &str,
    ) -> Result<Map<String, Value>, Box<dyn Error + Send + Sync>> {
        let tree = parse("json", text.as_bytes())?;
        Ok(to_config_map(&tree))
    }
}

impl FileStoredFormat for JsonFormat {
    fn file_extensions(&self) -> &'static [&'static str] {
        &["json"]
    }
}

/// A tree as a `config` table.
fn to_config_map(tree: &Tree) -> Map<String, Value> {
    tree.iter()
        .map(|(k, v)| (k.clone(), v.to_config_value(None)))
        .collect()
}
