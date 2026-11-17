//! Go `gopkg.in/yaml.v3` (v3.0.1) and `github.com/mitchellh/mapstructure` (v1.5.0) decoding
//! semantics, for reading Prebid Server's YAML files and viper configuration exactly as the Go
//! server does.
//!
//! - [`yaml`]: the libyaml scanner and parser, the composer and the decoder, with Go's values,
//!   leniencies and byte-identical error texts.
//! - [`Value`]: a Go `interface{}` tree (what `yaml.Unmarshal` into an `interface{}` produces, and
//!   what viper hands to mapstructure).
//! - [`mapstructure`]: decoding a [`Value`] tree into typed values, with viper's defaults
//!   ([`mapstructure::DecoderConfig::viper`]).
//! - [`GoYaml`] and [`GoMapstructure`]: derive macros for structs and named types, sharing the
//!   `#[goyaml(...)]` attribute and coexisting with [`pbs_gojson::GoJson`], whose `#[gojson(go_type
//!   = ...)]`, `go`, `skip` and `embedded` settings they also read.
//!
//! # Derive attributes
//!
//! Container (`#[goyaml(...)]` on the type):
//!
//! * `go_type = "pkg.Name"`: Go's `reflect.Type.String()`, used in error texts (default: the
//!   `#[gojson(go_type = ...)]` value; one of the two is required).
//!
//! Field (`#[goyaml(...)]` on a named field):
//!
//! * `go = "Name"`: the Go field name (default: `#[gojson(go = ...)]`, else the Rust name in
//!   `UpperCamelCase`). It decides exportedness, yaml.v3's default key (lowercased) and
//!   mapstructure's default key (matched case-insensitively).
//! * `yaml = "key,flags"`: the `yaml` struct tag (`-`, `omitempty`, `flow`, `inline`).
//! * `mapstructure = "key,flags"`: the `mapstructure` struct tag (`squash`, `remain`; other flags
//!   are ignored, as in Go).
//! * `embedded`: the Go field is embedded (default: `#[gojson(embedded)]`).
//! * `skip`: the field does not exist in Go (default: `#[gojson(skip)]`).
//!
//! A tuple struct with one field is a Go named type over the field's type (`type BidType
//! string`): it decodes as that type, under its own name. Its field type must implement
//! `Default`.
//!
//! # Example
//!
//! ```
//! use pbs_gojson::GoJson;
//! use pbs_goyaml::{GoMapstructure, GoYaml, Value, mapstructure, yaml};
//!
//! #[derive(Debug, Default, GoJson, GoYaml, GoMapstructure)]
//! #[gojson(go_type = "config.Syncer")]
//! struct Syncer {
//!     #[gojson(json = "key")]
//!     #[goyaml(yaml = "key", mapstructure = "key")]
//!     key: String,
//!     #[gojson(json = "supports")]
//!     #[goyaml(yaml = "supports", mapstructure = "supports")]
//!     supports: Vec<String>,
//!     #[gojson(json = "enabled")]
//!     #[goyaml(yaml = "enabled", mapstructure = "enabled")]
//!     enabled: Option<bool>,
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // yaml.v3 accepts YAML 1.1 booleans for a bool target.
//! let mut syncer = Syncer::default();
//! yaml::unmarshal(
//!     b"key: adnxs\nsupports: [iframe]\nenabled: yes\n",
//!     &mut syncer,
//! )?;
//! assert_eq!(syncer.enabled, Some(true), "yes is true");
//!
//! let mut bad = Syncer::default();
//! let err = yaml::unmarshal(b"supports: 3\n", &mut bad)
//!     .err()
//!     .map(|e| e.to_string());
//! assert_eq!(
//!     err.as_deref(),
//!     Some("yaml: unmarshal errors:\n  line 1: cannot unmarshal !!int `3` into []string"),
//!     "Go's type error",
//! );
//!
//! // viper's weak typing and string-to-slice hook.
//! let mut tree = Value::Nil;
//! yaml::unmarshal(b"key: 7\nsupports: iframe,redirect\n", &mut tree)?;
//! let mut decoded = Syncer::default();
//! mapstructure::Decoder::new(mapstructure::DecoderConfig::viper()).decode(&tree, &mut decoded)?;
//! assert_eq!(decoded.key, "7", "weakly typed");
//! assert_eq!(decoded.supports, ["iframe", "redirect"], "split on commas");
//! # Ok(())
//! # }
//! ```

#[doc(hidden)]
pub mod __private;
pub mod mapstructure;
mod value;
pub mod yaml;

use pbs_gocompat::{math, strconv};
pub use pbs_goyaml_derive::{GoMapstructure, GoYaml};
pub use value::{GENERAL_MAP, SLICE_OF_INTERFACE, STRING_MAP, Type, Value, kind_name};

/// `2^63` (Go's `float64(math.MaxInt64)`).
const TWO_POW_63: f64 = 9.223_372_036_854_776e18;
/// `2^64` (Go's `float64(math.MaxUint64)`).
const TWO_POW_64: f64 = 1.844_674_407_370_955_2e19;

/// Go's `uint64(f)` as compiled for amd64: values below `2^63` convert through `int64` (so
/// negative values wrap), larger ones through `int64(f - 2^63)` with the top bit set; NaN and
/// out-of-range values give `1<<63`.
fn go_uint64(f: f64) -> u64 {
    if f < TWO_POW_63 {
        u64::from_ne_bytes(math::int64(f).to_ne_bytes())
    } else {
        u64::from_ne_bytes(math::int64(f - TWO_POW_63).to_ne_bytes()) | (1 << 63)
    }
}

/// Go: `float64(v)` for a `uint64` (rounded to nearest, ties to even).
fn uint_to_float(v: u64) -> f64 {
    if let Ok(small) = u32::try_from(v) {
        return f64::from(small);
    }
    strconv::parse_float(&strconv::format_uint(v, 10), 64).unwrap_or(0.0)
}

/// Go: `float64(v)` for an `int64` (rounded to nearest, ties to even).
fn int_to_float(v: i64) -> f64 {
    if let Ok(small) = i32::try_from(v) {
        return f64::from(small);
    }
    strconv::parse_float(&strconv::format_int(v, 10), 64).unwrap_or(0.0)
}
