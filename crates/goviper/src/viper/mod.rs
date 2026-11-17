//! Go viper v1.12.0 `viper.go`, with `viper_go1_15.go` (the configuration file finder Prebid's
//! build uses) and the `json` and `yaml` decoders of `internal/encoding`.

mod filepath;
mod util;

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    error, fmt,
    fs::{self, File},
    io::{self, ErrorKind},
};

use pbs_gocompat::{
    strconv,
    strings::{Replacer, split, to_lower, to_upper},
    time::{Duration, Time},
};
use pbs_gojson::stdjson;
use pbs_goyaml::{
    Type, Value,
    mapstructure::{DecodeError, DecodeHook, Decoder, DecoderConfig, MsDecode},
    yaml,
};

use self::util::{
    StringMap, deep_set, insensitivise_map, parse_size_in_bytes, to_case_insensitive_value,
};
use crate::{
    cast::{self, StringMapStringSlice},
    env::Environment,
};

/// Go: `viper.SupportedExts`, in the order the finder tries them.
pub const SUPPORTED_EXTS: [&str; 12] = [
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

/// Go: the key delimiter (`v.keyDelim`), which Prebid never changes.
const KEY_DELIM: &str = ".";

/// The errors of viper's operations, whose `Display` is Go's `err.Error()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViperError {
    /// Go: `viper.ConfigFileNotFoundError`: no `<name>.<ext>` in any search path.
    ConfigFileNotFound {
        /// The configuration name searched for.
        name: String,
        /// The search paths, as Go's `fmt.Sprintf("%s", paths)` renders them (`[/a /b]`).
        locations: String,
    },
    /// Go: `viper.UnsupportedConfigError`: the configuration type is not one of
    /// [`SUPPORTED_EXTS`].
    UnsupportedConfig(String),
    /// Go: `viper.ConfigParseError`, by the decoder's error text (bytes, as yaml.v3's may cut a
    /// UTF-8 sequence).
    ConfigParse(Vec<u8>),
    /// Go: the `*fs.PathError` of reading the configuration file, by its text
    /// (`open /etc/config/pbs.yaml: permission denied`).
    Io(String),
    /// Go: the `BindEnv` error when no key is given.
    MissingKey,
    /// A configuration format viper supports but this port does not decode (TOML, HCL,
    /// `.properties`, dotenv and INI). Go would decode the file.
    UnportedFormat(String),
}

impl ViperError {
    /// The exact bytes of Go's `err.Error()` (for [`ViperError::UnportedFormat`], a Rust-only
    /// text).
    #[must_use]
    pub fn go_text(&self) -> Vec<u8> {
        match self {
            Self::ConfigFileNotFound { name, locations } => format!(
                "Config File {} Not Found in {}",
                strconv::quote(name),
                strconv::quote(locations)
            )
            .into_bytes(),
            Self::UnsupportedConfig(config_type) => {
                format!("Unsupported Config Type {}", strconv::quote(config_type)).into_bytes()
            }
            Self::ConfigParse(text) => {
                let mut out = b"While parsing config: ".to_vec();
                out.extend_from_slice(text);
                out
            }
            Self::Io(text) => text.clone().into_bytes(),
            Self::MissingKey => b"missing key to bind to".to_vec(),
            Self::UnportedFormat(format) => {
                format!("pbs-goviper: decoding {format} configuration is not ported").into_bytes()
            }
        }
    }
}

impl fmt::Display for ViperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.go_text()))
    }
}

impl error::Error for ViperError {}

/// Go: a `*viper.Viper`: a prioritized configuration registry, with Go's lookup order (override,
/// environment, configuration file, default), case-insensitive keys and `.`-separated paths.
///
/// See the [crate documentation](crate) for what is ported.
#[derive(Debug, Clone)]
pub struct Viper {
    /// Go: `configPaths`.
    config_paths: Vec<String>,
    /// Go: `configName`.
    config_name: String,
    /// Go: `configFile`.
    config_file: String,
    /// Go: `configType`.
    config_type: String,
    /// Go: `envPrefix`.
    env_prefix: String,
    /// Go: `automaticEnvApplied`.
    automatic_env_applied: bool,
    /// Go: `envKeyReplacer`.
    env_key_replacer: Option<Replacer>,
    /// Go: `allowEmptyEnv`.
    allow_empty_env: bool,
    /// Go: `config`.
    config: StringMap,
    /// Go: `override`.
    overrides: StringMap,
    /// Go: `defaults`.
    defaults: StringMap,
    /// Go: `env`: each key's bound variable names, in binding order.
    env: BTreeMap<String, Vec<String>>,
    /// Go: `typeByDefValue`.
    type_by_def_value: bool,
    /// Where Go's `os` package reads from.
    environment: Environment,
}

impl Default for Viper {
    fn default() -> Self {
        Self::new()
    }
}

impl Viper {
    /// Go: `viper.New()`, reading the process environment.
    #[must_use]
    pub fn new() -> Self {
        Self::with_environment(Environment::process())
    }

    /// Go: `viper.New()`, reading `environment` instead of the process's variables and working
    /// directory.
    #[must_use]
    pub fn with_environment(environment: Environment) -> Self {
        Self {
            config_paths: Vec::new(),
            config_name: "config".to_owned(),
            config_file: String::new(),
            config_type: String::new(),
            env_prefix: String::new(),
            automatic_env_applied: false,
            env_key_replacer: None,
            allow_empty_env: false,
            config: BTreeMap::new(),
            overrides: BTreeMap::new(),
            defaults: BTreeMap::new(),
            env: BTreeMap::new(),
            type_by_def_value: false,
            environment,
        }
    }

    /// The environment this instance reads.
    #[must_use]
    pub const fn environment(&self) -> &Environment {
        &self.environment
    }

    // ---- configuration file ------------------------------------------------------------------

    /// Go: `SetConfigFile`: read this file instead of searching (ignored when empty).
    pub fn set_config_file(&mut self, file: &str) {
        if !file.is_empty() {
            file.clone_into(&mut self.config_file);
        }
    }

    /// Go: `ConfigFileUsed`: the file set or found, or `""`.
    #[must_use]
    pub fn config_file_used(&self) -> &str {
        &self.config_file
    }

    /// Go: `SetConfigName`: the file name (without extension) to search for; forgets a file set
    /// or found before (ignored when empty).
    pub fn set_config_name(&mut self, name: &str) {
        if !name.is_empty() {
            name.clone_into(&mut self.config_name);
            self.config_file.clear();
        }
    }

    /// Go: `SetConfigType`: the format to decode, instead of the file's extension (ignored when
    /// empty).
    pub fn set_config_type(&mut self, config_type: &str) {
        if !config_type.is_empty() {
            config_type.clone_into(&mut self.config_type);
        }
    }

    /// Go: `AddConfigPath`: a directory to search, made absolute (`$HOME` and `$VAR`s expanded,
    /// relative paths joined to the working directory); a path already added is not added again
    /// (ignored when empty).
    pub fn add_config_path(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        let absin = self.abs_pathify(path);
        if !self.config_paths.contains(&absin) {
            self.config_paths.push(absin);
        }
    }

    /// The search paths, in the order they are searched.
    #[must_use]
    pub fn config_paths(&self) -> &[String] {
        &self.config_paths
    }

    /// Go: `ReadInConfig`: finds the configuration file (or takes the one set) and replaces the
    /// configuration with its contents. On error the configuration is left as it was.
    ///
    /// # Errors
    ///
    /// Go's: [`ViperError::ConfigFileNotFound`], [`ViperError::UnsupportedConfig`],
    /// [`ViperError::Io`] or [`ViperError::ConfigParse`] (and [`ViperError::UnportedFormat`]
    /// for a format this port does not decode).
    pub fn read_in_config(&mut self) -> Result<(), ViperError> {
        let filename = self.get_config_file()?;
        let config_type = self.get_config_type();
        if !SUPPORTED_EXTS.contains(&config_type.as_str()) {
            return Err(ViperError::UnsupportedConfig(config_type));
        }
        let file = self.read_file(&filename)?;
        let mut config = BTreeMap::new();
        self.unmarshal_reader(&file, &mut config)?;
        self.config = config;
        Ok(())
    }

    /// Go: `ReadConfig(bytes.NewReader(data))`: replaces the configuration with `data` decoded
    /// as the configuration type (set, or from the configuration file's extension; with neither,
    /// nothing is decoded). On a decoding error the configuration holds what was decoded before
    /// it, with its keys as written.
    ///
    /// # Errors
    ///
    /// Go's [`ViperError::ConfigParse`] (and [`ViperError::UnportedFormat`] for a format this
    /// port does not decode).
    pub fn read_config(&mut self, data: &[u8]) -> Result<(), ViperError> {
        self.config = BTreeMap::new();
        let mut config = BTreeMap::new();
        let result = self.unmarshal_reader(data, &mut config);
        self.config = config;
        result
    }

    // ---- environment ---------------------------------------------------------------------------

    /// Go: `SetEnvPrefix`: the prefix (joined with `_`) of the variables `BindEnv` with one
    /// argument and `AutomaticEnv` read (ignored when empty).
    pub fn set_env_prefix(&mut self, prefix: &str) {
        if !prefix.is_empty() {
            prefix.clone_into(&mut self.env_prefix);
        }
    }

    /// Go: `SetEnvKeyReplacer`: rewrites variable names before they are looked up. `None`
    /// removes the replacer, leaving names as a new viper does (in Go, passing a nil
    /// `*strings.Replacer` makes the next lookup panic).
    pub fn set_env_key_replacer(&mut self, replacer: Option<Replacer>) {
        self.env_key_replacer = replacer;
    }

    /// Go: `AutomaticEnv`: every lookup also reads the variable named after the key (prefixed,
    /// uppercased, and through the replacer).
    pub const fn automatic_env(&mut self) {
        self.automatic_env_applied = true;
    }

    /// Go: `AllowEmptyEnv`: whether a variable set to `""` counts as set (it does not by
    /// default).
    pub const fn allow_empty_env(&mut self, allow: bool) {
        self.allow_empty_env = allow;
    }

    /// Go: `BindEnv(input...)`: binds the key `input[0]` to the variables `input[1..]` (read in
    /// that order), or with one argument to the key itself, prefixed and uppercased as the prefix
    /// is now.
    ///
    /// # Errors
    ///
    /// [`ViperError::MissingKey`] when `input` is empty.
    pub fn bind_env(&mut self, input: &[&str]) -> Result<(), ViperError> {
        let Some((first, rest)) = input.split_first() else {
            return Err(ViperError::MissingKey);
        };
        let key = to_lower(first);
        if rest.is_empty() {
            let name = self.merge_with_env_prefix(&key);
            self.env.entry(key).or_default().push(name);
        } else {
            self.env
                .entry(key)
                .or_default()
                .extend(rest.iter().map(|name| (*name).to_owned()));
        }
        Ok(())
    }

    // ---- values
    // ----------------------------------------------------------------------------------

    /// Go: `SetTypeByDefaultValue`: [`Viper::get`] converts values to the type of the key's
    /// default (as a string set in the environment becomes a `[]string` for a `[]string`
    /// default).
    pub const fn set_type_by_default_value(&mut self, enable: bool) {
        self.type_by_def_value = enable;
    }

    /// Go: `SetDefault`: the value used when no other source has the key. Map values are copied
    /// with their keys lowercased.
    pub fn set_default(&mut self, key: &str, value: Value) {
        let key = to_lower(key);
        let value = to_case_insensitive_value(value);
        let path = split(&key, KEY_DELIM);
        if let Some((last, parents)) = path.split_last() {
            deep_set(&mut self.defaults, parents, to_lower(last), value);
        }
    }

    /// Go: `Set`: an override, used before every other source. Map values are copied with their
    /// keys lowercased.
    pub fn set(&mut self, key: &str, value: Value) {
        let key = to_lower(key);
        let value = to_case_insensitive_value(value);
        let path = split(&key, KEY_DELIM);
        if let Some((last, parents)) = path.split_last() {
            deep_set(&mut self.overrides, parents, to_lower(last), value);
        }
    }

    /// Go: `Get`: the key's value from the first source that has it (`None` for Go's nil),
    /// converted to its default's type under [`Viper::set_type_by_default_value`].
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Value> {
        let lcase_key = to_lower(key);
        let val = self.find(&lcase_key)?;
        if self.type_by_def_value {
            let path = split(&lcase_key, KEY_DELIM);
            let val_type = search_map(&self.defaults, &path).unwrap_or(&*val);
            if let Some(converted) = convert_to_type_of(val_type, &val) {
                return Some(converted);
            }
        }
        Some(val.into_owned())
    }

    /// The value of [`Viper::get`], with Go's nil as [`Value::Nil`].
    fn get_or_nil(&self, key: &str) -> Value {
        self.get(key).unwrap_or_default()
    }

    /// Go: `GetString`.
    #[must_use]
    pub fn get_string(&self, key: &str) -> String {
        cast::to_string(&self.get_or_nil(key))
    }

    /// Go: `GetBool`.
    #[must_use]
    pub fn get_bool(&self, key: &str) -> bool {
        cast::to_bool(&self.get_or_nil(key))
    }

    /// Go: `GetInt` (Go's `int` is 64 bits).
    #[must_use]
    pub fn get_int(&self, key: &str) -> i64 {
        cast::to_int(&self.get_or_nil(key))
    }

    /// Go: `GetInt32`.
    #[must_use]
    pub fn get_int32(&self, key: &str) -> i32 {
        cast::to_int32(&self.get_or_nil(key))
    }

    /// Go: `GetInt64`.
    #[must_use]
    pub fn get_int64(&self, key: &str) -> i64 {
        cast::to_int64(&self.get_or_nil(key))
    }

    /// Go: `GetUint` (Go's `uint` is 64 bits).
    #[must_use]
    pub fn get_uint(&self, key: &str) -> u64 {
        cast::to_uint(&self.get_or_nil(key))
    }

    /// Go: `GetUint32`.
    #[must_use]
    pub fn get_uint32(&self, key: &str) -> u32 {
        cast::to_uint32(&self.get_or_nil(key))
    }

    /// Go: `GetUint64`.
    #[must_use]
    pub fn get_uint64(&self, key: &str) -> u64 {
        cast::to_uint64(&self.get_or_nil(key))
    }

    /// Go: `GetFloat64`.
    #[must_use]
    pub fn get_float64(&self, key: &str) -> f64 {
        cast::to_float64(&self.get_or_nil(key))
    }

    /// Go: `GetTime`.
    #[must_use]
    pub fn get_time(&self, key: &str) -> Time {
        cast::to_time(&self.get_or_nil(key))
    }

    /// Go: `GetDuration`.
    #[must_use]
    pub fn get_duration(&self, key: &str) -> Duration {
        cast::to_duration(&self.get_or_nil(key))
    }

    /// Go: `GetIntSlice`; `None` is a nil slice.
    #[must_use]
    pub fn get_int_slice(&self, key: &str) -> Option<Vec<i64>> {
        cast::to_int_slice(&self.get_or_nil(key))
    }

    /// Go: `GetStringSlice`; `None` is a nil slice.
    #[must_use]
    pub fn get_string_slice(&self, key: &str) -> Option<Vec<String>> {
        cast::to_string_slice(&self.get_or_nil(key))
    }

    /// Go: `GetStringMap`; `None` is a nil map.
    #[must_use]
    pub fn get_string_map(&self, key: &str) -> Option<BTreeMap<String, Value>> {
        cast::to_string_map(&self.get_or_nil(key))
    }

    /// Go: `GetStringMapString`; `None` is a nil map.
    #[must_use]
    pub fn get_string_map_string(&self, key: &str) -> Option<BTreeMap<String, String>> {
        cast::to_string_map_string(&self.get_or_nil(key))
    }

    /// Go: `GetStringMapStringSlice`; `None` is a nil map.
    #[must_use]
    pub fn get_string_map_string_slice(&self, key: &str) -> Option<StringMapStringSlice> {
        cast::to_string_map_string_slice(&self.get_or_nil(key))
    }

    /// Go: `GetSizeInBytes` (Go's `uint` is 64 bits).
    #[must_use]
    pub fn get_size_in_bytes(&self, key: &str) -> u64 {
        parse_size_in_bytes(&cast::to_string(&self.get_or_nil(key)))
    }

    /// Go: `IsSet`: whether any source has a non-nil value for the key (defaults included).
    #[must_use]
    pub fn is_set(&self, key: &str) -> bool {
        self.find(&to_lower(key)).is_some()
    }

    /// Go: `InConfig`: whether the configuration file has the key.
    #[must_use]
    pub fn in_config(&self, key: &str) -> bool {
        let lcase_key = to_lower(key);
        let path = split(&lcase_key, KEY_DELIM);
        search_indexable(Indexable::Root(&self.config), &path).is_some()
    }

    /// Go: `AllKeys`: every key holding a value (or bound to a variable), `.`-joined, sorted
    /// (Go's order is random).
    #[must_use]
    pub fn all_keys(&self) -> Vec<String> {
        let mut shadow = BTreeSet::new();
        // Go: "add all paths, by order of descending priority to ensure correct shadowing".
        flatten_and_merge_map(&mut shadow, &self.overrides, "");
        merge_flat_map(&mut shadow, self.env.keys());
        flatten_and_merge_map(&mut shadow, &self.config, "");
        flatten_and_merge_map(&mut shadow, &self.defaults, "");
        shadow.into_iter().collect()
    }

    /// Go: `AllSettings`: the value of every key of [`Viper::all_keys`] that has one, as nested
    /// maps.
    ///
    /// The keys are processed in sorted order. When one key is a path prefix of another, Go's
    /// result can depend on its random iteration order; this is one of its outcomes.
    #[must_use]
    pub fn all_settings(&self) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        for k in self.all_keys() {
            let Some(value) = self.get(&k) else {
                continue;
            };
            let path = split(&k, KEY_DELIM);
            if let Some((last, parents)) = path.split_last() {
                deep_set(&mut m, parents, to_lower(last), value);
            }
        }
        m
    }

    /// Go: `Sub`: a new instance (reading the same environment) whose configuration is the map
    /// at `key`, or `None` when the key holds no map.
    #[must_use]
    pub fn sub(&self, key: &str) -> Option<Self> {
        let data = self.get(key)?;
        if !matches!(data, Value::StringMap(..) | Value::Map(..)) {
            return None;
        }
        let mut subv = Self::with_environment(self.environment.clone());
        subv.config = cast::to_string_map(&data).unwrap_or_default();
        Some(subv)
    }

    /// Go: `Unmarshal(rawVal)`: decodes [`Viper::all_settings`] into `out` with viper's default
    /// decoder configuration ([`DecoderConfig::viper`]).
    ///
    /// # Errors
    ///
    /// Go's mapstructure error.
    pub fn unmarshal<T: MsDecode + ?Sized>(&self, out: &mut T) -> Result<(), DecodeError> {
        self.unmarshal_with(out, DecoderConfig::viper())
    }

    /// Go: `Unmarshal(rawVal, opts...)`, with the options already applied to the default
    /// configuration (see [`decode_hook`]).
    ///
    /// # Errors
    ///
    /// Go's mapstructure error.
    pub fn unmarshal_with<T: MsDecode + ?Sized>(
        &self,
        out: &mut T,
        config: DecoderConfig,
    ) -> Result<(), DecodeError> {
        let input = Value::StringMap(&Type::Interface, Some(self.all_settings()));
        Decoder::new(config).decode(&input, out)
    }

    /// Go: `UnmarshalExact(rawVal)`: [`Viper::unmarshal`], failing on settings no field takes.
    ///
    /// # Errors
    ///
    /// Go's mapstructure error.
    pub fn unmarshal_exact<T: MsDecode + ?Sized>(&self, out: &mut T) -> Result<(), DecodeError> {
        let config = DecoderConfig {
            error_unused: true,
            ..DecoderConfig::viper()
        };
        self.unmarshal_with(out, config)
    }

    /// Go: `UnmarshalKey(key, rawVal)`: decodes [`Viper::get`] of `key` into `out` with viper's
    /// default decoder configuration (a missing key leaves `out` alone).
    ///
    /// # Errors
    ///
    /// Go's mapstructure error.
    pub fn unmarshal_key<T: MsDecode + ?Sized>(
        &self,
        key: &str,
        out: &mut T,
    ) -> Result<(), DecodeError> {
        self.unmarshal_key_with(key, out, DecoderConfig::viper())
    }

    /// Go: `UnmarshalKey(key, rawVal, opts...)`, with the options already applied to the
    /// default configuration.
    ///
    /// # Errors
    ///
    /// Go's mapstructure error.
    pub fn unmarshal_key_with<T: MsDecode + ?Sized>(
        &self,
        key: &str,
        out: &mut T,
        config: DecoderConfig,
    ) -> Result<(), DecodeError> {
        Decoder::new(config).decode(&self.get_or_nil(key), out)
    }

    // ---- internals
    // -------------------------------------------------------------------------------

    /// Go: `find(lcaseKey, flagDefault)`: the first non-nil value in override, environment,
    /// configuration and defaults, stopping where a higher-priority source has a scalar on the
    /// key's path.
    fn find(&self, lcase_key: &str) -> Option<Cow<'_, Value>> {
        let path = split(lcase_key, KEY_DELIM);
        let nested = path.len() > 1;

        // Set() override first
        if let Some(val) = search_map(&self.overrides, &path) {
            return Some(Cow::Borrowed(val));
        }
        if nested && is_path_shadowed_in_deep_map(&path, &self.overrides) {
            return None;
        }

        // Env override next
        if self.automatic_env_applied {
            // even if it hasn't been registered, if automaticEnv is used, check any Get request
            if let Some(val) = self.get_env(&self.merge_with_env_prefix(lcase_key)) {
                return Some(Cow::Owned(Value::String(val)));
            }
            if nested && self.is_path_shadowed_in_auto_env(&path) {
                return None;
            }
        }
        if let Some(envkeys) = self.env.get(lcase_key) {
            for envkey in envkeys {
                if let Some(val) = self.get_env(envkey) {
                    return Some(Cow::Owned(Value::String(val)));
                }
            }
        }
        // Go: isPathShadowedInFlatMap(path, v.env) is always "": it only handles
        // map[string]string and map[string]FlagValue, and v.env is a map[string][]string.

        // Config file next
        if let Some(val) = search_indexable(Indexable::Root(&self.config), &path) {
            return Some(Cow::Borrowed(val));
        }
        if nested && is_path_shadowed_in_deep_map(&path, &self.config) {
            return None;
        }

        // Default next (the key/value store and flags are always empty).
        search_map(&self.defaults, &path).map(Cow::Borrowed)
    }

    /// Go: `mergeWithEnvPrefix`.
    fn merge_with_env_prefix(&self, key: &str) -> String {
        if self.env_prefix.is_empty() {
            return to_upper(key);
        }
        to_upper(&format!("{}_{key}", self.env_prefix))
    }

    /// Go: `getEnv`: the variable (its name through the replacer), if set (and non-empty, unless
    /// empty values are allowed).
    fn get_env(&self, key: &str) -> Option<String> {
        let replaced = self.env_key_replacer.as_ref().map(|r| r.replace(key));
        self.environment
            .lookup(replaced.as_deref().unwrap_or(key))
            .filter(|val| self.allow_empty_env || !val.is_empty())
    }

    /// Go: `isPathShadowedInAutoEnv`: whether a variable is set for a proper prefix of the path.
    fn is_path_shadowed_in_auto_env(&self, path: &[&str]) -> bool {
        (1..path.len()).any(|i| {
            let parent_key = path.get(..i).unwrap_or_default().join(KEY_DELIM);
            self.get_env(&self.merge_with_env_prefix(&parent_key))
                .is_some()
        })
    }

    /// Go: `absPathify`.
    fn abs_pathify(&self, path: &str) -> String {
        let home_expanded = if path == "$HOME" || path.starts_with("$HOME/") {
            let rest = path.get(5..).unwrap_or_default();
            Cow::Owned(format!("{}{rest}", self.environment.getenv("HOME")))
        } else {
            Cow::Borrowed(path)
        };
        let in_path = self.environment.expand(&home_expanded);
        if filepath::is_abs(&in_path) {
            return filepath::clean(&in_path);
        }
        // Go: filepath.Abs joins the path to os.Getwd(); on an error viper logs it and adds "".
        self.environment.getwd().map_or_else(String::new, |wd| {
            filepath::clean(&filepath::join(&[&wd, &in_path]))
        })
    }

    /// Go: `getConfigFile`: the file set, or the one found (which is then remembered).
    fn get_config_file(&mut self) -> Result<String, ViperError> {
        if self.config_file.is_empty() {
            self.config_file = self.find_config_file()?;
        }
        Ok(self.config_file.clone())
    }

    /// Go: `getConfigType`: the type set, or the configuration file's extension, or `""`.
    fn get_config_type(&mut self) -> String {
        if !self.config_type.is_empty() {
            return self.config_type.clone();
        }
        let Ok(cf) = self.get_config_file() else {
            return String::new();
        };
        let ext = filepath::ext(&cf);
        ext.get(1..).unwrap_or_default().to_owned()
    }

    /// Go: `findConfigFile` (`viper_go1_15.go`): the first search path holding the file.
    fn find_config_file(&self) -> Result<String, ViperError> {
        for cp in &self.config_paths {
            let file = self.search_in_path(cp);
            if !file.is_empty() {
                return Ok(file);
            }
        }
        Err(ViperError::ConfigFileNotFound {
            name: self.config_name.clone(),
            locations: format!("[{}]", self.config_paths.join(" ")),
        })
    }

    /// Go: `searchInPath`: `<name>.<ext>` for each supported extension in order, then (with a
    /// configuration type set) `<name>` itself.
    fn search_in_path(&self, dir: &str) -> String {
        for ext in SUPPORTED_EXTS {
            let file = filepath::join(&[dir, &format!("{}.{ext}", self.config_name)]);
            if self.exists(&file) {
                return file;
            }
        }
        if !self.config_type.is_empty() {
            let file = filepath::join(&[dir, &self.config_name]);
            if self.exists(&file) {
                return file;
            }
        }
        String::new()
    }

    /// Go: `exists`: whether `stat` succeeds on something that is not a directory.
    fn exists(&self, path: &str) -> bool {
        fs::metadata(self.environment.resolve(path)).is_ok_and(|m| !m.is_dir())
    }

    /// Go: `afero.ReadFile` on the OS file system.
    fn read_file(&self, filename: &str) -> Result<Vec<u8>, ViperError> {
        let path = self.environment.resolve(filename);
        let mut file =
            File::open(&path).map_err(|e| ViperError::Io(path_error("open", filename, &e)))?;
        let mut data = Vec::new();
        let _read = io::copy(&mut file, &mut data)
            .map_err(|e| ViperError::Io(path_error("read", filename, &e)))?;
        Ok(data)
    }

    /// Go: `unmarshalReader`: decodes `data` into `c` by the configuration type, then lowercases
    /// its keys.
    fn unmarshal_reader(&mut self, data: &[u8], c: &mut StringMap) -> Result<(), ViperError> {
        let format = to_lower(&self.get_config_type());
        match format.as_str() {
            // Go: yaml.Unmarshal(b, &v) into the map[string]interface{}.
            "yaml" | "yml" => {
                yaml::unmarshal(data, c).map_err(|e| ViperError::ConfigParse(e.text()))?;
            }
            // Go: json.Unmarshal(b, &v) into the map[string]interface{}.
            "json" => {
                let mut decoded: Option<BTreeMap<String, serde_json::Value>> =
                    Some(BTreeMap::new());
                let result = stdjson::unmarshal(data, &mut decoded);
                c.extend(
                    decoded
                        .into_iter()
                        .flatten()
                        .map(|(k, v)| (k, cast::from_json(v))),
                );
                result.map_err(|e| ViperError::ConfigParse(e.go_text()))?;
            }
            "toml" | "hcl" | "tfvars" | "ini" | "properties" | "props" | "prop" | "dotenv"
            | "env" => return Err(ViperError::UnportedFormat(format)),
            _ => {}
        }
        insensitivise_map(c);
        Ok(())
    }
}

/// Go: viper's `DecodeHook(hook)` option applied to the default decoder configuration: `hook`
/// replaces viper's own hooks (Prebid's `config.New` passes `AccountModulesHookFunc` this way).
#[must_use]
pub fn decode_hook(hook: DecodeHook) -> DecoderConfig {
    DecoderConfig {
        decode_hook: Some(hook),
        ..DecoderConfig::viper()
    }
}

/// Go: the text of an `*fs.PathError` (`op path: err`), with Go's texts for the common errnos.
fn path_error(op: &str, path: &str, e: &io::Error) -> String {
    let kind = e.kind();
    let text = if kind == ErrorKind::NotFound {
        "no such file or directory".to_owned()
    } else if kind == ErrorKind::PermissionDenied {
        "permission denied".to_owned()
    } else if kind == ErrorKind::IsADirectory {
        "is a directory".to_owned()
    } else if kind == ErrorKind::NotADirectory {
        "not a directory".to_owned()
    } else if kind == ErrorKind::InvalidFilename {
        "file name too long".to_owned()
    } else {
        // Go's syscall texts are the C library's, lowercased; Rust appends " (os error N)".
        let rust = e.to_string();
        let base = rust.split(" (os error").next().unwrap_or_default();
        let mut chars = base.chars();
        chars.next().map_or_else(String::new, |first| {
            first.to_lowercase().chain(chars).collect()
        })
    };
    format!("{op} {path}: {text}")
}

/// Go's `case map[interface{}]interface{}` and `case map[string]interface{}`.
const fn is_map(v: &Value) -> bool {
    matches!(
        v,
        Value::StringMap(Type::Interface, _) | Value::Map(Type::Interface, Type::Interface, _)
    )
}

/// The value of a `map[interface{}]interface{}` under the key `ToString` turns into `key`,
/// as `cast.ToStringMap` keeps it: the last such entry.
fn general_get<'a>(entries: Option<&'a Vec<(Value, Value)>>, key: &str) -> Option<&'a Value> {
    entries?
        .iter()
        .rev()
        .find(|(k, _)| cast::to_string(k) == key)
        .map(|(_, v)| v)
}

/// The value one level down in a Go map value (`None` for a value that is not a map).
fn map_child<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::StringMap(Type::Interface, entries) => entries.as_ref()?.get(key),
        Value::Map(Type::Interface, Type::Interface, entries) => general_get(entries.as_ref(), key),
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::String(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => None,
    }
}

/// Go: `searchMap(source, path)` for a non-empty path; `None` for Go's nil.
fn search_map<'a>(source: &'a StringMap, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut next = source.get(*first)?;
    for key in rest {
        // Go: "got a value but nested key expected, return nil for not found".
        next = map_child(next, key)?;
    }
    (!next.is_nil()).then_some(next)
}

/// Go: `isPathShadowedInDeepMap`: whether a proper prefix of the path holds a value that is not
/// a map.
fn is_path_shadowed_in_deep_map(path: &[&str], m: &StringMap) -> bool {
    for i in 1..path.len() {
        let Some(parent_val) = search_map(m, path.get(..i).unwrap_or_default()) else {
            // not found, no need to add more path elements
            return false;
        };
        if !is_map(parent_val) {
            // parentVal is a regular value which shadows "path"
            return true;
        }
    }
    false
}

/// A source of `searchIndexableWithPathPrefixes`.
#[derive(Debug, Clone, Copy)]
enum Indexable<'a> {
    /// The configuration's root `map[string]interface{}`.
    Root(&'a StringMap),
    /// A nested value: Go searches `map[string]interface{}`, `[]interface{}` and (through
    /// `cast.ToStringMap`) `map[interface{}]interface{}` values.
    Nested(&'a Value),
}

impl<'a> Indexable<'a> {
    /// The value under `prefix_key` (a map key, or a slice index for Go's `strconv.Atoi`).
    fn lookup(self, prefix_key: &str) -> Option<&'a Value> {
        match self {
            Self::Root(m) => m.get(prefix_key),
            Self::Nested(Value::Slice(Type::Interface, items)) => {
                // Go: "if the prefixKey is not a number or it is out of bounds of the slice".
                // A negative index makes Go panic; it finds nothing here.
                let index = strconv::atoi(prefix_key).ok()?;
                items.as_ref()?.get(usize::try_from(index).ok()?)
            }
            Self::Nested(v) => map_child(v, prefix_key),
        }
    }
}

/// Go: `searchIndexableWithPathPrefixes`: the value at the path, trying the longest `.`-joined
/// prefix as one key first (a key `"foo.bar"` beats `foo` → `bar`); `None` for Go's nil.
fn search_indexable<'a>(source: Indexable<'a>, path: &[&str]) -> Option<&'a Value> {
    // search for path prefixes, starting from the longest one
    for i in (1..=path.len()).rev() {
        let prefix_key = to_lower(&path.get(..i).unwrap_or_default().join(KEY_DELIM));
        let Some(next) = source.lookup(&prefix_key) else {
            continue;
        };
        let found = if i == path.len() {
            // Fast path
            (!next.is_nil()).then_some(next)
        } else if is_map(next) || matches!(next, Value::Slice(Type::Interface, _)) {
            // Nested case
            search_indexable(Indexable::Nested(next), path.get(i..).unwrap_or_default())
        } else {
            // got a value but nested key expected, do nothing and look for next prefix
            None
        };
        if found.is_some() {
            return found;
        }
    }
    // not found
    None
}

/// Go: `flattenAndMergeMap`: adds the lowercased `.`-joined path of every non-map value in `m`
/// to `shadow`, skipping a map whose prefix is already a key there.
fn flatten_and_merge_map(shadow: &mut BTreeSet<String>, m: &StringMap, prefix: &str) {
    if !prefix.is_empty() && shadow.contains(prefix) {
        // prefix is shadowed => nothing more to flatten
        return;
    }
    for (k, val) in m {
        let full_key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}{KEY_DELIM}{k}")
        };
        match val {
            Value::StringMap(Type::Interface, nested) => {
                let empty = BTreeMap::new();
                let nested = nested.as_ref().unwrap_or(&empty);
                flatten_and_merge_map(shadow, nested, &full_key);
            }
            Value::Map(Type::Interface, Type::Interface, _) => {
                let nested = cast::to_string_map(val).unwrap_or_default();
                flatten_and_merge_map(shadow, &nested, &full_key);
            }
            Value::Nil
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Int8(_)
            | Value::Int16(_)
            | Value::Int32(_)
            | Value::Int64(_)
            | Value::Uint(_)
            | Value::Uint8(_)
            | Value::Uint16(_)
            | Value::Uint32(_)
            | Value::Uint64(_)
            | Value::Float32(_)
            | Value::Float64(_)
            | Value::String(_)
            | Value::Time(_)
            | Value::Duration(_)
            | Value::Slice(..)
            | Value::StringMap(..)
            | Value::Map(..) => {
                // immediate value
                let _new = shadow.insert(to_lower(&full_key));
            }
        }
    }
}

/// Go: `mergeFlatMap`: adds each key whose proper prefixes are not keys in `shadow` (keys are
/// taken in sorted order, so of two bound keys where one prefixes the other only the shorter is
/// added; Go's order is random).
fn merge_flat_map<'a, I>(shadow: &mut BTreeSet<String>, keys: I)
where
    I: Iterator<Item = &'a String>,
{
    'outer: for k in keys {
        let path = split(k, KEY_DELIM);
        // scan intermediate paths
        for i in 1..path.len() {
            let parent_key = path.get(..i).unwrap_or_default().join(KEY_DELIM);
            if shadow.contains(&parent_key) {
                // path is shadowed, continue
                continue 'outer;
            }
        }
        let _new = shadow.insert(to_lower(k));
    }
}

/// Go: the `typeByDefValue` switch of `Get`: `val` converted to the type of `val_type`, or
/// `None` when that type is none of the switch's cases (the value is then returned as it is).
fn convert_to_type_of(val_type: &Value, val: &Value) -> Option<Value> {
    Some(match val_type {
        Value::Bool(_) => Value::Bool(cast::to_bool(val)),
        Value::String(_) => Value::String(cast::to_string(val)),
        Value::Int32(_) | Value::Int16(_) | Value::Int8(_) | Value::Int(_) => {
            Value::Int(cast::to_int(val))
        }
        Value::Uint(_) => Value::Uint(cast::to_uint(val)),
        Value::Uint32(_) => Value::Uint32(cast::to_uint32(val)),
        Value::Uint64(_) => Value::Uint64(cast::to_uint64(val)),
        Value::Int64(_) => Value::Int64(cast::to_int64(val)),
        Value::Float64(_) | Value::Float32(_) => Value::Float64(cast::to_float64(val)),
        Value::Time(_) => Value::Time(cast::to_time(val)),
        Value::Duration(_) => Value::Duration(cast::to_duration(val)),
        Value::Slice(Type::String, _) => Value::Slice(
            &Type::String,
            cast::to_string_slice(val).map(|items| items.into_iter().map(Value::String).collect()),
        ),
        Value::Slice(Type::Int, _) => Value::Slice(
            &Type::Int,
            cast::to_int_slice(val).map(|items| items.into_iter().map(Value::Int).collect()),
        ),
        Value::Nil
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => return None,
    })
}
