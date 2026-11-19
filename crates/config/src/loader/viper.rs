//! Glue (Rule B (c)): how `github.com/spf13/viper` v1.12.0 resolves a key over the layers
//! `config.SetupViper` gives it (config/config.go:936-1415): which value `Get` returns, which
//! keys `AllKeys` lists and the tree `AllSettings` builds from them, which `v.Unmarshal` decodes.
//! These decide the operator's interface: which `PBS_` variable or file key wins, what a set
//! parent variable or a file value hides, and the Go type a value reaches the decoder with
//! (ledger: `config.layering`, `config.key-case`, `config.weak-typing`).
//!
//! The functions follow viper.go's `find`, `searchMap`, `searchIndexableWithPathPrefixes`,
//! `isPathShadowedInDeepMap`, `isPathShadowedInAutoEnv`, `Get`, `IsSet`, `AllKeys`,
//! `flattenAndMergeMap`, `mergeFlatMap`, `AllSettings` and `deepSearch`, and util.go's
//! `insensitiviseMap`. Where Go's result depends on the order it iterates a map in, these iterate
//! in sorted key order, one of the orders Go can take (ledger: `config.map-order`).

use std::collections::{BTreeMap, BTreeSet};

use pbs_gocompat::{
    strconv::atoi,
    strings::{to_lower, to_upper},
};

use super::{
    cast,
    defaults::{BIDDER_KEYS, DEFAULTS, UNPREFIXED_BINDINGS},
    env::EnvLayer,
    value::GoValue,
};

/// The layer a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Layer {
    /// `v.SetDefault`.
    Defaults,
    /// The configuration file.
    File,
    /// A `PBS_` variable or a `BindEnv` fallback.
    Env,
}

/// A tree of Viper values, keyed as Viper keys it.
pub(super) type Tree = BTreeMap<String, GoValue>;

/// Viper as `config.SetupViper` leaves it.
#[derive(Debug, Clone)]
pub(super) struct Viper {
    /// `v.defaults`.
    defaults: Tree,
    /// `v.config`, keys lower-cased (`insensitiviseMap`).
    config: Tree,
    /// The environment.
    env: EnvLayer,
    /// `v.env`: each `BindEnv` key and the variables bound to it, in binding order.
    bound: BTreeMap<String, Vec<String>>,
}

/// `v.AllSettings()` and the layer each value in it came from.
#[derive(Debug, Clone)]
pub(super) struct AllSettings {
    /// The tree.
    pub(super) tree: Tree,
    /// Each key `AllSettings` set, in the order it set them, and its value's layer.
    pub(super) writes: Vec<(Vec<String>, Layer)>,
}

impl Viper {
    /// Viper after `config.SetupViper(v, "pbs", bidders)` read `config` as its file.
    pub(super) fn new(config: Tree, env: EnvLayer, bidders: &[String]) -> Self {
        let mut defaults = Tree::new();
        for (key, default) in DEFAULTS {
            set_default(&mut defaults, key, default.value());
        }
        let mut bound: BTreeMap<String, Vec<String>> = BTreeMap::new();
        // Go: BindEnv(key) before SetEnvPrefix binds strings.ToUpper(key) (viper.go
        // mergeWithEnvPrefix with no prefix).
        for key in UNPREFIXED_BINDINGS {
            let key = to_lower(key);
            let name = to_upper(&key);
            bound.entry(key).or_default().push(name);
        }
        // Go: setBidderDefaults(v, strings.ToLower(bidderName)) after SetEnvPrefix("PBS").
        for bidder in bidders {
            let bidder = to_lower(bidder);
            for suffix in BIDDER_KEYS {
                let key = to_lower(&format!("adapters.{bidder}.{suffix}"));
                let name = merge_with_env_prefix(&key);
                bound.entry(key).or_default().push(name);
            }
        }
        Self {
            defaults,
            config,
            env,
            bound,
        }
    }

    /// The configuration file's tree as Viper keeps it.
    pub(super) const fn config(&self) -> &Tree {
        &self.config
    }

    /// Go: `getEnv`: the variable after the `.` to `_` replacer, unless it is unset or empty.
    fn get_env(&self, name: &str) -> Option<GoValue> {
        self.env
            .get(&name.replace('.', "_"))
            .map(|v| GoValue::String(v.to_owned()))
    }

    /// Go: `find(lcaseKey, flagDefault)`: the value of the first layer that has the key, `None`
    /// for Go's nil.
    fn find(&self, key: &str) -> Option<(GoValue, Layer)> {
        let path: Vec<&str> = key.split('.').collect();
        let nested = path.len() > 1;
        // AutomaticEnv: PBS_<KEY> for every key.
        if let Some(v) = self.get_env(&merge_with_env_prefix(key)) {
            return Some((v, Layer::Env));
        }
        if nested && self.is_path_shadowed_in_auto_env(&path) {
            return None;
        }
        if let Some(names) = self.bound.get(key) {
            for name in names {
                if let Some(v) = self.get_env(name) {
                    return Some((v, Layer::Env));
                }
            }
        }
        if let Some(v) = search_indexable_with_path_prefixes_map(&self.config, &path) {
            return Some((v, Layer::File));
        }
        if nested && is_path_shadowed_in_deep_map(&path, &self.config) {
            return None;
        }
        if let Some(v) = search_map(&self.defaults, &path) {
            return Some((v, Layer::Defaults));
        }
        None
    }

    /// Go: `isPathShadowedInAutoEnv`: whether a `PBS_` variable is set for a parent of the key.
    fn is_path_shadowed_in_auto_env(&self, path: &[&str]) -> bool {
        (1..path.len()).any(|i| {
            path.get(..i).is_some_and(|parent| {
                self.get_env(&merge_with_env_prefix(&parent.join(".")))
                    .is_some()
            })
        })
    }

    /// Go: `v.Get(key)` with `SetTypeByDefaultValue(true)`: the value cast to the type of the
    /// key's default when that is a type Viper casts to.
    pub(super) fn get(&self, key: &str) -> Option<(GoValue, Layer)> {
        let key = to_lower(key);
        let (value, layer) = self.find(&key)?;
        let path: Vec<&str> = key.split('.').collect();
        let value = match search_map(&self.defaults, &path) {
            Some(default) => cast_like(&default, value),
            None => value,
        };
        Some((value, layer))
    }

    /// Go: `v.IsSet(key)`.
    pub(super) fn is_set(&self, key: &str) -> bool {
        self.find(&to_lower(key)).is_some()
    }

    /// Go: `v.AllKeys()`, sorted.
    pub(super) fn all_keys(&self) -> BTreeSet<String> {
        let mut shadow = BTreeSet::new();
        // Go: mergeFlatMap(m, v.env).
        for key in self.bound.keys() {
            let path: Vec<&str> = key.split('.').collect();
            let shadowed = (1..path.len())
                .any(|i| path.get(..i).is_some_and(|p| shadow.contains(&p.join("."))));
            if !shadowed {
                let _new = shadow.insert(to_lower(key));
            }
        }
        flatten_and_merge(&mut shadow, &self.config, "");
        flatten_and_merge(&mut shadow, &self.defaults, "");
        shadow
    }

    /// Go: `v.AllSettings()`.
    pub(super) fn all_settings(&self) -> AllSettings {
        let mut tree = Tree::new();
        let mut writes = Vec::new();
        for key in self.all_keys() {
            let Some((value, layer)) = self.get(&key) else {
                continue;
            };
            let path: Vec<String> = key.split('.').map(str::to_owned).collect();
            let Some((last, parents)) = path.split_last() else {
                continue;
            };
            insert_at(&mut tree, parents, to_lower(last), value);
            writes.push((path, layer));
        }
        AllSettings { tree, writes }
    }
}

impl AllSettings {
    /// The layer of the value at `path` in the tree: the layer of the last value `AllSettings`
    /// set at the path or above it.
    pub(super) fn layer_of(&self, path: &[String]) -> Layer {
        self.writes
            .iter()
            .rev()
            .find(|(written, _)| path.starts_with(written))
            .map_or(Layer::Defaults, |(_, layer)| *layer)
    }
}

/// Go: `mergeWithEnvPrefix` after `SetEnvPrefix("PBS")`.
pub(super) fn merge_with_env_prefix(key: &str) -> String {
    to_upper(&format!("PBS_{key}"))
}

/// Go: `v.SetDefault(key, value)` (`deepSearch` and the innermost assignment).
fn set_default(defaults: &mut Tree, key: &str, value: GoValue) {
    let key = to_lower(key);
    let path: Vec<String> = key.split('.').map(str::to_owned).collect();
    if let Some((last, parents)) = path.split_last() {
        insert_at(defaults, parents, to_lower(last), value);
    }
}

/// Go: `deepSearch(m, parents)[last] = value`: a parent that is missing or holds a value that is
/// not a `map[string]interface{}` becomes a new map.
fn insert_at(tree: &mut Tree, parents: &[String], last: String, value: GoValue) {
    match parents.split_first() {
        None => {
            let _previous = tree.insert(last, value);
        }
        Some((first, rest)) => {
            let slot = tree
                .entry(first.clone())
                .or_insert_with(|| GoValue::Map(Tree::new()));
            if !matches!(slot, GoValue::Map(_)) {
                *slot = GoValue::Map(Tree::new());
            }
            if let GoValue::Map(next) = slot {
                insert_at(next, rest, last, value);
            }
        }
    }
}

/// Go: `cast.ToStringMap` of a `map[interface{}]interface{}`: keys through `cast.ToString`.
pub(super) fn to_string_map(entries: &[(GoValue, GoValue)]) -> Tree {
    entries
        .iter()
        .map(|(k, v)| (cast::to_string(k), v.clone()))
        .collect()
}

/// Go: `Get`'s cast by the default's type (viper.go `Get`, the `typeByDefValue` switch).
fn cast_like(default: &GoValue, value: GoValue) -> GoValue {
    match default {
        GoValue::Bool(_) => GoValue::Bool(cast::to_bool(&value)),
        GoValue::String(_) => GoValue::String(cast::to_string(&value)),
        GoValue::Int(_) => GoValue::Int(cast::to_int(&value)),
        GoValue::Float64(_) => GoValue::Float64(cast::to_float64(&value)),
        GoValue::Strings(_) => GoValue::Strings(cast::to_string_slice(&value)),
        GoValue::Nil
        | GoValue::Int64(_)
        | GoValue::Uint64(_)
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => value,
    }
}

/// Go: `searchMap(source, path)`: `None` for a missing key, a nil value, or a path through a
/// value that is not a map.
fn search_map(source: &Tree, path: &[&str]) -> Option<GoValue> {
    let (first, rest) = path.split_first()?;
    let next = source.get(*first)?;
    if rest.is_empty() {
        return non_nil(next);
    }
    match next {
        GoValue::Map(map) => search_map(map, rest),
        GoValue::AnyMap(entries) => search_map(&to_string_map(entries), rest),
        GoValue::Nil
        | GoValue::Bool(_)
        | GoValue::Int(_)
        | GoValue::Int64(_)
        | GoValue::Uint64(_)
        | GoValue::Float64(_)
        | GoValue::String(_)
        | GoValue::List(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => None,
    }
}

/// `Some(v)` unless `v` is nil.
fn non_nil(v: &GoValue) -> Option<GoValue> {
    if matches!(v, GoValue::Nil) {
        None
    } else {
        Some(v.clone())
    }
}

/// Go: `searchIndexableWithPathPrefixes(source, path)` for a map source: the longest literal
/// dotted prefix first, numeric segments indexing lists.
fn search_indexable_with_path_prefixes_map(source: &Tree, path: &[&str]) -> Option<GoValue> {
    for i in (1..=path.len()).rev() {
        let prefix = path.get(..i)?;
        let key = to_lower(&prefix.join("."));
        let Some(next) = source.get(&key) else {
            continue;
        };
        if let Some(v) = search_next(next, path.get(i..)?) {
            return Some(v);
        }
    }
    None
}

/// Go: `searchIndexableWithPathPrefixes(source, path)` for a list source.
fn search_indexable_with_path_prefixes_list(source: &[GoValue], path: &[&str]) -> Option<GoValue> {
    for i in (1..=path.len()).rev() {
        let prefix = path.get(..i)?;
        let key = to_lower(&prefix.join("."));
        // Go: strconv.Atoi(prefixKey); a negative index panics in Go and finds nothing here.
        let Some(next) = atoi(&key)
            .ok()
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| source.get(index))
        else {
            continue;
        };
        if let Some(v) = search_next(next, path.get(i..)?) {
            return Some(v);
        }
    }
    None
}

/// Go: the tail of `searchMapWithPathPrefixes` and `searchSliceWithPathPrefixes`: the value
/// itself when the path ends, else the search continues in a map or a list.
fn search_next(next: &GoValue, rest: &[&str]) -> Option<GoValue> {
    if rest.is_empty() {
        return non_nil(next);
    }
    match next {
        GoValue::Map(map) => search_indexable_with_path_prefixes_map(map, rest),
        GoValue::AnyMap(entries) => {
            search_indexable_with_path_prefixes_map(&to_string_map(entries), rest)
        }
        GoValue::List(items) => search_indexable_with_path_prefixes_list(items, rest),
        GoValue::Nil
        | GoValue::Bool(_)
        | GoValue::Int(_)
        | GoValue::Int64(_)
        | GoValue::Uint64(_)
        | GoValue::Float64(_)
        | GoValue::String(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => None,
    }
}

/// Go: `isPathShadowedInDeepMap(path, m)`: whether a parent of the key holds a value that is not
/// a map.
fn is_path_shadowed_in_deep_map(path: &[&str], m: &Tree) -> bool {
    for i in 1..path.len() {
        let Some(parent) = path.get(..i).and_then(|p| search_map(m, p)) else {
            return false;
        };
        if !parent.is_map() {
            return true;
        }
    }
    false
}

/// Go: `flattenAndMergeMap(shadow, m, prefix)`.
fn flatten_and_merge(shadow: &mut BTreeSet<String>, m: &Tree, prefix: &str) {
    if !prefix.is_empty() && shadow.contains(prefix) {
        return;
    }
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}.")
    };
    for (key, value) in m {
        let full = format!("{prefix}{key}");
        match value {
            GoValue::Map(map) => flatten_and_merge(shadow, map, &full),
            GoValue::AnyMap(entries) => flatten_and_merge(shadow, &to_string_map(entries), &full),
            GoValue::Nil
            | GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_)
            | GoValue::List(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_) => {
                let _new = shadow.insert(to_lower(&full));
            }
        }
    }
}

/// Go: util.go's `insensitiviseMap`: every key of the tree, and of the maps below it through
/// maps (not through lists), lower-cased, a `map[interface{}]interface{}` converted with
/// `cast.ToStringMap`. A key that is not lower case overwrites its lower-case spelling.
pub(super) fn insensitivise_map(m: &mut Tree) {
    let keys: Vec<String> = m.keys().cloned().collect();
    for key in keys {
        // Go reads each value when its key is reached, after earlier keys may have replaced it.
        let Some(value) = m.remove(&key) else {
            continue;
        };
        let value = match value {
            GoValue::AnyMap(entries) => {
                let mut map = to_string_map(&entries);
                insensitivise_map(&mut map);
                GoValue::Map(map)
            }
            GoValue::Map(mut map) => {
                insensitivise_map(&mut map);
                GoValue::Map(map)
            }
            other @ (GoValue::Nil
            | GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_)
            | GoValue::List(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_)) => other,
        };
        let _previous = m.insert(to_lower(&key), value);
    }
}
