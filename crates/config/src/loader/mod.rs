//! The host configuration loader: Prebid Server's configuration read as Go reads it, with
//! `config.SetupViper` (config/config.go:936) and the decode of `config.New`
//! (config/config.go:756-760).
//!
//! The loader builds a [`config::Config`] from three layers, as `config.SetupViper` sets Viper
//! up: the defaults `SetupViper` registers, the first `pbs.<ext>` found in `.` and then
//! `/etc/config` (one file, never merged), and the `PBS_` environment with the un-prefixed
//! `BindEnv` fallbacks. Which layer gives a key its value, the Go type it reaches the decoder
//! with, and the keys a layer hides follow Viper v1.12.0 (Rule B (c), the operator's
//! interface): keys are lower-cased, a value is cast to the type of its key's default
//! (`cast.ToBool`, `ToInt`, ...), and a key bound only with `BindEnv` reaches the decoder as the
//! string the variable holds. [`Settings::decode`] then runs mapstructure's weak decode into the
//! Go types of `config.Configuration`, which decides whether the configuration loads.
//!
//! The environment is an input ([`Loader::env`]), a snapshot the loader never reads from the
//! process itself.
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pbs_config::loader::{GoValue, Loader};
//!
//! let env = BTreeMap::from([("PBS_PORT".to_owned(), "9000".to_owned())]);
//! let settings = Loader::new().without_file().env(env).load()?;
//! assert_eq!(settings.get("port"), Some(GoValue::Int(9000)));
//! assert!(settings.decode().is_ok());
//! # Ok::<(), pbs_config::loader::LoadError>(())
//! ```

mod cast;
mod decode;
mod defaults;
mod env;
mod file;
#[cfg(test)]
mod tests;
mod value;
mod viper;

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use config::{Config, ConfigError, Map, Source, Value, ValueKind};

pub use self::{
    decode::Decoded,
    file::{JsonFormat, SUPPORTED_EXTS, YamlFormat, yaml_options},
    value::GoValue,
};
use self::{
    decode::{CONFIGURATION, Decoder, configuration_types},
    env::EnvLayer,
    file::Found,
    viper::{AllSettings, Layer, Tree, Viper},
};

/// Go: yaml.v3's `Unmarshal(b, &map[string]interface{})`, as Viper's YAML codec reads `pbs.yaml`.
///
/// The first document's mapping with its keys as written (the loader then lower-cases them),
/// each value in the Go type yaml.v3's untyped resolution gives it. An empty or null document is
/// an empty map.
///
/// # Errors
///
/// The reader's error text when the document is not YAML or not a mapping.
pub fn read_yaml(bytes: &[u8]) -> Result<BTreeMap<String, GoValue>, String> {
    file::parse_yaml(bytes)
}

/// Where the loader looks for its file, and the environment and bidders it reads with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loader {
    /// The configuration file's name without extension; `None` reads no file.
    name: Option<String>,
    /// The directories searched, in order.
    search_paths: Vec<PathBuf>,
    /// The environment snapshot.
    env: BTreeMap<String, String>,
    /// The bidders whose `adapters.<bidder>.*` keys are bound.
    bidders: Vec<String>,
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

impl Loader {
    /// The loader as `main.go` sets Viper up (`config.SetupViper(v, "pbs", bidderInfos)`):
    /// `pbs.<ext>` in `.` and then `/etc/config`, with an empty environment and no bidders.
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: Some("pbs".to_owned()),
            search_paths: vec![PathBuf::from("."), PathBuf::from("/etc/config")],
            env: BTreeMap::new(),
            bidders: Vec::new(),
        }
    }

    /// Searches `paths` in order instead of `.` and `/etc/config`.
    #[must_use]
    pub fn search_paths<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.search_paths = paths.into_iter().map(Into::into).collect();
        self
    }

    /// Reads no configuration file (`config.SetupViper(v, "", bidderInfos)`).
    #[must_use]
    pub fn without_file(mut self) -> Self {
        self.name = None;
        self
    }

    /// Reads the environment from `snapshot` (variable name to value).
    #[must_use]
    pub fn env(mut self, snapshot: BTreeMap<String, String>) -> Self {
        self.env = snapshot;
        self
    }

    /// Binds the `adapters.<bidder>.*` keys of each of `bidders`, as `setBidderDefaults` does
    /// for every bidder of `static/bidder-info`.
    #[must_use]
    pub fn bidders<I, S>(mut self, bidders: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.bidders = bidders.into_iter().map(Into::into).collect();
        self
    }

    /// Loads the configuration.
    ///
    /// A file that cannot be read or parsed is ignored, as `config.SetupViper` ignores the error
    /// of `ReadInConfig`; [`Settings::read_error`] keeps it.
    ///
    /// # Errors
    ///
    /// [`LoadError::UnsupportedFormat`] when the file found is in a format Viper reads and this
    /// port does not (properties, HCL, dotenv or INI), and [`LoadError::Config`] when the
    /// `config` crate fails to read the environment or build the configuration.
    pub fn load(&self) -> Result<Settings, LoadError> {
        let (tree, file, read_error) = match &self.name {
            None => (Tree::new(), None, None),
            Some(name) => match file::search(name, &self.search_paths) {
                Found::Missing(text) => (Tree::new(), None, Some(text)),
                Found::Unsupported { path, ext } => {
                    return Err(LoadError::UnsupportedFormat { path, ext });
                }
                Found::Read { path, dir, result } => match result {
                    Ok(tree) => (tree, Some((path, dir)), None),
                    Err(text) => (Tree::new(), Some((path, dir)), Some(text)),
                },
            },
        };
        let env = EnvLayer::new(&self.env).map_err(LoadError::Config)?;
        let viper = Viper::new(tree, env, &self.bidders);
        let all = viper.all_settings();
        let origin = file.as_ref().map(|(path, _)| path.display().to_string());
        let config = build_config(&all, origin).map_err(LoadError::Config)?;
        Ok(Settings {
            viper,
            all,
            config,
            file,
            read_error,
        })
    }
}

/// The loaded configuration.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Viper's state.
    viper: Viper,
    /// `v.AllSettings()`.
    all: AllSettings,
    /// The tree as a `config::Config`.
    config: Config,
    /// The file read and the index of its search path.
    file: Option<(PathBuf, usize)>,
    /// The error `ReadInConfig` returned.
    read_error: Option<String>,
}

impl Settings {
    /// The configuration as a [`config::Config`]: Viper's `AllSettings` tree, each value with
    /// the origin of its layer (the defaults, the file's path, or the environment).
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Go: `v.AllSettings()`, the tree `v.Unmarshal` decodes.
    #[must_use]
    pub const fn all_settings(&self) -> &BTreeMap<String, GoValue> {
        &self.all.tree
    }

    /// Go: `v.AllKeys()`, sorted.
    #[must_use]
    pub fn all_keys(&self) -> Vec<String> {
        self.viper.all_keys().into_iter().collect()
    }

    /// Go: `v.Get(key)`; `None` for Go's nil.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<GoValue> {
        self.viper.get(key).map(|(value, _)| value)
    }

    /// Go: `v.IsSet(key)`.
    #[must_use]
    pub fn is_set(&self, key: &str) -> bool {
        self.viper.is_set(key)
    }

    /// The configuration file read, if one was found.
    #[must_use]
    pub fn file_used(&self) -> Option<&Path> {
        self.file.as_ref().map(|(path, _)| path.as_path())
    }

    /// The index in the search paths of the directory the file was found in.
    #[must_use]
    pub fn file_search_index(&self) -> Option<usize> {
        self.file.as_ref().map(|(_, dir)| *dir)
    }

    /// The error Viper's `ReadInConfig` returned, which `config.SetupViper` ignores: no file
    /// found, or one that could not be read or parsed (its values are then not used).
    #[must_use]
    pub fn read_error(&self) -> Option<&str> {
        self.read_error.as_deref()
    }

    /// The file's tree as Viper keeps it (keys lower-cased).
    #[must_use]
    pub const fn file_tree(&self) -> &BTreeMap<String, GoValue> {
        self.viper.config()
    }

    /// Go: `v.Unmarshal(&c, viper.DecodeHook(config.AccountModulesHookFunc()))`
    /// (config/config.go:758): the configuration decoded into the Go types of
    /// `config.Configuration`.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when mapstructure's weak decode fails, as `config.New` then fails.
    pub fn decode(&self) -> Result<Decoded, DecodeError> {
        let types = configuration_types().map_err(|message| DecodeError { message })?;
        let decoder = Decoder::new(types, true);
        let mut out = decoder.zero(CONFIGURATION).map_err(|e| DecodeError {
            message: e.to_string(),
        })?;
        decoder
            .decode(
                "",
                &GoValue::Map(self.all.tree.clone()),
                CONFIGURATION,
                &mut out,
            )
            .map_err(|e| DecodeError {
                message: e.to_string(),
            })?;
        Ok(out)
    }

    /// Go: `v.UnmarshalKey(key, &b)` into a `*bool` (config/config.go:886-889, for
    /// `adapters.<bidder>.disabled` and `.modifyingvastxmlallowed`): `Ok(None)` when the key has
    /// no value.
    ///
    /// # Errors
    ///
    /// mapstructure's error text, which Go wraps as `viper failed to unmarshal bidder config
    /// disabled: <text>`.
    pub fn unmarshal_key_bool(&self, key: &str) -> Result<Option<bool>, String> {
        let types = configuration_types()?;
        let decoder = Decoder::new(types, false);
        let mut out = Decoded::Ptr(None);
        let input = self.get(key).unwrap_or(GoValue::Nil);
        decoder
            .decode("", &input, "*bool", &mut out)
            .map_err(|e| e.to_string())?;
        Ok(match out.target() {
            Some(Decoded::Bool(b)) => Some(*b),
            Some(_) | None => None,
        })
    }
}

/// Why the configuration did not load.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// The configuration file found is in a format Viper reads and this port does not
    /// (D-VIPERFMT): Java properties, HCL, dotenv or INI.
    UnsupportedFormat {
        /// The file.
        path: PathBuf,
        /// Its extension.
        ext: &'static str,
    },
    /// The `config` crate failed.
    Config(ConfigError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { path, ext } => write!(
                f,
                "the configuration file {} is in the {ext} format, which this Prebid Server does \
                 not read: use pbs.json, pbs.toml, pbs.yaml or pbs.yml",
                path.display()
            ),
            Self::Config(err) => write!(f, "reading the configuration: {err}"),
        }
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnsupportedFormat { .. } => None,
            Self::Config(err) => Some(err),
        }
    }
}

/// The configuration did not decode into `config.Configuration`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    /// mapstructure's error text.
    message: String,
}

impl DecodeError {
    /// mapstructure's error text, without `config.New`'s wrapper.
    #[must_use]
    pub fn mapstructure_text(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DecodeError {
    /// Go: `config.New`'s error (config/config.go:759).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "viper failed to unmarshal app config: {}", self.message)
    }
}

impl Error for DecodeError {}

/// One layer's values as a `config` source.
#[derive(Debug, Clone)]
struct LayerSource {
    /// The origin its values carry.
    origin: String,
    /// The leaves of the tree it gives, by path.
    leaves: Vec<(Vec<String>, GoValue)>,
}

impl Source for LayerSource {
    fn clone_into_box(&self) -> Box<dyn Source + Send + Sync> {
        Box::new(self.clone())
    }

    fn collect(&self) -> Result<Map<String, Value>, ConfigError> {
        let mut root = Value::new(None, ValueKind::Table(Map::new()));
        self.collect_to(&mut root)?;
        Ok(match root.kind {
            ValueKind::Table(map) => map,
            ValueKind::Nil
            | ValueKind::Boolean(_)
            | ValueKind::I64(_)
            | ValueKind::I128(_)
            | ValueKind::U64(_)
            | ValueKind::U128(_)
            | ValueKind::Float(_)
            | ValueKind::String(_)
            | ValueKind::Array(_) => Map::new(),
        })
    }

    /// Sets each leaf at its path, whose keys are segments, not `config` path expressions.
    fn collect_to(&self, cache: &mut Value) -> Result<(), ConfigError> {
        for (path, value) in &self.leaves {
            set_leaf(cache, path, value.to_config_value(Some(&self.origin)));
        }
        Ok(())
    }
}

/// Sets `value` at `path` under `cache`, making tables of the parents.
fn set_leaf(cache: &mut Value, path: &[String], value: Value) {
    let Some((first, rest)) = path.split_first() else {
        *cache = value;
        return;
    };
    if !matches!(cache.kind, ValueKind::Table(_)) {
        cache.kind = ValueKind::Table(Map::new());
    }
    if let ValueKind::Table(map) = &mut cache.kind {
        let child = map
            .entry(first.clone())
            .or_insert_with(|| Value::new(None, ValueKind::Table(Map::new())));
        set_leaf(child, rest, value);
    }
}

/// The leaves of a tree: values that are not maps with entries.
fn leaves(tree: &Tree, prefix: &[String], out: &mut Vec<(Vec<String>, GoValue)>) {
    for (key, value) in tree {
        let mut path = prefix.to_vec();
        path.push(key.clone());
        match value {
            GoValue::Map(map) if !map.is_empty() => leaves(map, &path, out),
            GoValue::Nil
            | GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_)
            | GoValue::List(_)
            | GoValue::Map(_)
            | GoValue::AnyMap(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_) => out.push((path, value.clone())),
        }
    }
}

/// The `config::Config` of the tree: a source per layer, each with the leaves it gave.
fn build_config(all: &AllSettings, file_origin: Option<String>) -> Result<Config, ConfigError> {
    let mut all_leaves = Vec::new();
    leaves(&all.tree, &[], &mut all_leaves);
    let mut defaults = Vec::new();
    let mut file = Vec::new();
    let mut env = Vec::new();
    for (path, value) in all_leaves {
        match all.layer_of(&path) {
            Layer::Defaults => defaults.push((path, value)),
            Layer::File => file.push((path, value)),
            Layer::Env => env.push((path, value)),
        }
    }
    Config::builder()
        .add_source(LayerSource {
            origin: "the defaults".to_owned(),
            leaves: defaults,
        })
        .add_source(LayerSource {
            origin: file_origin.unwrap_or_else(|| "the configuration file".to_owned()),
            leaves: file,
        })
        .add_source(LayerSource {
            origin: "the environment".to_owned(),
            leaves: env,
        })
        .build()
}
