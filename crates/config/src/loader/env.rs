//! Glue (Rule B (c)): the environment layer of `config.SetupViper` (config/config.go:1318-1321
//! and its `BindEnv` calls): `PBS_` plus the upper-case key with `.` replaced by `_`, the
//! un-prefixed `BindEnv` fallbacks such as `GDPR_DEFAULT_VALUE`, and an empty value read as unset
//! (Viper's `AllowEmptyEnv` is off; ledger: `config.env-names`).
//!
//! The environment is an input: the loader takes a snapshot of it, which `config`'s
//! [`Environment`] source reads, so a test hands the loader a map and never sets a process
//! variable. Only the variables that can reach a key are read: `PBS_` ones and the five
//! fallbacks. Go looks variables up by their exact name; `config` lower-cases the names it
//! collects, so the layer keeps each variable under its own name and uses `config` to read the
//! snapshot and drop the empty ones.

use std::collections::BTreeMap;

use config::{ConfigError, Environment, Map, Source as _};
use pbs_gocompat::strings::to_upper;

use super::defaults::UNPREFIXED_BINDINGS;

/// The variables of the environment snapshot Viper can read, by exact name; none is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct EnvLayer {
    vars: BTreeMap<String, String>,
}

impl EnvLayer {
    /// Reads `snapshot` as Viper reads the process environment.
    pub(super) fn new(snapshot: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        let fallbacks: Vec<String> = UNPREFIXED_BINDINGS
            .iter()
            .map(|key| to_upper(key).replace('.', "_"))
            .collect();
        let wanted: BTreeMap<String, String> = snapshot
            .iter()
            .filter(|(name, _)| name.starts_with("PBS_") || fallbacks.contains(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let source: Map<String, String> = wanted.clone().into_iter().collect();
        let collected = Environment::default()
            .source(Some(source))
            .ignore_empty(true)
            .collect()?;
        let vars = wanted
            .into_iter()
            // `config` keys what it collected by the lower-cased name, which two variables can
            // share; each keeps its own value, and an empty one is unset whatever its twin holds.
            .filter(|(name, value)| {
                !value.is_empty() && collected.contains_key(&name.to_lowercase())
            })
            .collect();
        Ok(Self { vars })
    }

    /// The value of the variable `name`, unless it is unset or empty.
    pub(super) fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }
}
