//! `spf13/viper` v1.12 configuration semantics used by Prebid Server.
//!
//! [`Viper`] ports the `*viper.Viper` behaviour Prebid's `config.SetupViper`, `config.New` and
//! `main.loadConfig` rely on, over [`pbs_goyaml`]'s [`Value`](pbs_goyaml::Value) trees (Go's
//! `interface{}` values) and its mapstructure decoder:
//!
//! - **Sources, in priority order**: overrides ([`Viper::set`]), environment variables
//!   ([`Viper::automatic_env`], [`Viper::bind_env`] with one name or several, a prefix and a
//!   [`Replacer`](pbs_gocompat::strings::Replacer), empty values ignored unless
//!   [`Viper::allow_empty_env`]), the configuration file, and defaults ([`Viper::set_default`]). A
//!   scalar on a key's path in a higher-priority source hides the key in the lower ones, as in Go.
//! - **Keys** are case-insensitive and `.`-separated; configuration keys may also contain dots
//!   (`"a.b": 1` is found for `a.b`) and index `[]interface{}` items (`tv.0.title`).
//! - **Reads**: [`Viper::get`] (with [`Viper::set_type_by_default_value`]'s conversion to the
//!   default's type), every typed getter with `spf13/cast` v1.5.0's conversions ([`cast`]),
//!   [`Viper::is_set`], [`Viper::in_config`], [`Viper::all_keys`], [`Viper::all_settings`],
//!   [`Viper::sub`], and [`Viper::unmarshal`]/[`Viper::unmarshal_key`] with viper's default decoder
//!   configuration (or [`decode_hook`]'s, as `viper.DecodeHook` makes it).
//! - **Configuration files**: [`Viper::read_in_config`] searches each [`Viper::add_config_path`]
//!   directory for `<name>.<ext>` over [`SUPPORTED_EXTS`] in Go's order (or reads
//!   [`Viper::set_config_file`]'s file), decodes JSON with `encoding/json` and YAML with `yaml.v3`,
//!   and lowercases the keys; [`Viper::read_config`] decodes bytes. Errors carry Go's texts
//!   ([`ViperError::go_text`]), such as `Config File "pbs" Not Found in "[/app /etc/config]"`.
//!
//! Environment variables and the working directory come from an [`Environment`]: the process's
//! ([`Viper::new`]) or given ones ([`Viper::with_environment`]).
//!
//! # Example
//!
//! ```
//! use pbs_gocompat::strings::Replacer;
//! use pbs_goviper::{Environment, Viper};
//! use pbs_goyaml::Value;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let env = Environment::from_vars([("PBS_PORT", "7777"), ("PBS_HOST_COOKIE_TTL_DAYS", "")]);
//! let mut v = Viper::with_environment(env);
//! v.set_default("port", Value::Int(8000));
//! v.set_default("host_cookie.ttl_days", Value::Int(90));
//! v.set_env_key_replacer(Some(Replacer::new([(".", "_")])));
//! v.set_type_by_default_value(true);
//! v.set_env_prefix("PBS");
//! v.automatic_env();
//! v.read_config(b"")?;
//!
//! // The variable's string, converted to the default's type.
//! assert_eq!(
//!     v.get("PORT"),
//!     Some(Value::Int(7777)),
//!     "from the environment"
//! );
//! // An empty variable is ignored.
//! assert_eq!(v.get_int("host_cookie.ttl_days"), 90, "the default");
//! # Ok(())
//! # }
//! ```
//!
//! # Not ported
//!
//! Nothing Prebid uses is left out; of the rest of viper:
//!
//! - Decoding TOML, HCL, `.properties`, dotenv and INI files: such a file is found in Go's search
//!   order, but reading it is a [`ViperError::UnportedFormat`] error.
//! - Aliases, pflags, remote key/value stores, `Merge*`, `Write*`, `WatchConfig`, the
//!   `KeyDelimiter` option (the delimiter is always `.`), `afero` file systems other than the OS's,
//!   and the package-level functions over the global instance.
//! - Go's random map iteration: [`Viper::all_keys`] is sorted; where Go's result depends on the
//!   iteration order (keys that differ only in case, a bound key that is a prefix of another,
//!   [`Viper::all_settings`] with a key that is a path prefix of another), this port gives one of
//!   Go's outcomes.
//! - Go's aliasing: [`Viper::get`] returns copies, so [`Viper::all_settings`] never writes into a
//!   map another source holds (Go's can, when a key is a path prefix of another).
//! - Go panics: a negative slice index in a key (`tv.-1`) finds nothing instead, and
//!   [`Viper::set_env_key_replacer`] takes `None` (Go's nil `*strings.Replacer` panics on use).
//! - Go named types: a [`Value`](pbs_goyaml::Value) cannot hold `[]openrtb_ext.BidderName` or
//!   `[]config.AgmaAnalyticsAccount`; an empty `[]interface{}` default stands for such an empty
//!   slice (it converts, decodes and encodes alike).

pub mod cast;
mod env;
mod viper;

pub use env::Environment;
pub use viper::{SUPPORTED_EXTS, Viper, ViperError, decode_hook};
