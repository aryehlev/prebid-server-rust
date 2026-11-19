//! Go's dynamic values as Viper keeps them.
//!
//! Viper's layers hold whatever the file readers produced (`map[string]interface{}` trees of
//! `bool`, `int`, `int64`, `uint64`, `float64`, `string` and `[]interface{}`), its casts produce
//! `[]string` (a nil one when nothing could be cast), and `config.SetupViper` registers a few
//! defaults of slice types no cast touches. The decoders and the error texts tell all of these
//! apart, so [`GoValue`] keeps each one.

use std::{collections::BTreeMap, fmt};

use config::{Value, ValueKind};
use serde_json::{Map as JsonMap, Number, Value as Json};

use super::{cast, goconv::float_text};

/// A value of Go's `interface{}` as Viper holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum GoValue {
    /// `nil`: a YAML or JSON `null`, or a key nothing sets.
    Nil,
    /// `bool`.
    Bool(bool),
    /// `int` (64 bits on amd64): yaml.v3's integers and `cast.ToInt`'s results.
    Int(i64),
    /// `int64`: go-toml's integers.
    Int64(i64),
    /// `uint64`: yaml.v3's integers above the `int` range.
    Uint64(u64),
    /// `float64`: encoding/json's numbers and the floats of the other readers.
    Float64(f64),
    /// `string`.
    String(String),
    /// `[]interface{}`.
    List(Vec<Self>),
    /// `map[string]interface{}`.
    Map(BTreeMap<String, Self>),
    /// `map[interface{}]interface{}`: a yaml.v3 mapping whose keys are not all strings, which
    /// Viper converts to [`GoValue::Map`] except inside a list.
    AnyMap(Vec<(Self, Self)>),
    /// `[]string`; `None` is a nil slice, which `cast.ToStringSlice` returns for a value it
    /// cannot cast.
    Strings(Option<Vec<String>>),
    /// `[][]string` (the `user_sync.priority_groups` default).
    StringLists(Vec<Vec<String>>),
    /// An empty slice of a named element type that `config.SetupViper` registers as a default,
    /// by its Go type (`[]config.AgmaAnalyticsAccount`, `[]openrtb_ext.BidderName`).
    EmptySlice(&'static str),
}

impl GoValue {
    /// Go: `fmt.Sprintf("%T", v)`.
    #[must_use]
    pub const fn go_type(&self) -> &'static str {
        match self {
            Self::Nil => "<nil>",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Int64(_) => "int64",
            Self::Uint64(_) => "uint64",
            Self::Float64(_) => "float64",
            Self::String(_) => "string",
            Self::List(_) => "[]interface {}",
            Self::Map(_) => "map[string]interface {}",
            Self::AnyMap(_) => "map[interface {}]interface {}",
            Self::Strings(_) => "[]string",
            Self::StringLists(_) => "[][]string",
            Self::EmptySlice(ty) => ty,
        }
    }

    /// Go: the reflect kind mapstructure and Viper's `deepSearch` branch on, as `Kind.String()`
    /// names it (`int`, `int64`, `uint64`, `float64`, `slice`, `map`, ...).
    #[must_use]
    pub const fn go_kind(&self) -> &'static str {
        match self {
            Self::Nil => "invalid",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Int64(_) => "int64",
            Self::Uint64(_) => "uint64",
            Self::Float64(_) => "float64",
            Self::String(_) => "string",
            Self::List(_) | Self::Strings(_) | Self::StringLists(_) | Self::EmptySlice(_) => {
                "slice"
            }
            Self::Map(_) | Self::AnyMap(_) => "map",
        }
    }

    /// Whether Go's `reflect.Value.Kind()` of the value is `Slice`.
    #[must_use]
    pub const fn is_slice(&self) -> bool {
        matches!(
            self,
            Self::List(_) | Self::Strings(_) | Self::StringLists(_) | Self::EmptySlice(_)
        )
    }

    /// Whether Go's `reflect.Value.Kind()` of the value is `Map`.
    #[must_use]
    pub const fn is_map(&self) -> bool {
        matches!(self, Self::Map(_) | Self::AnyMap(_))
    }

    /// The elements of a slice, as `reflect.Value.Index(i).Interface()` gives them; `None` for a
    /// value that is not a slice or a nil `[]string`.
    #[must_use]
    pub fn elements(&self) -> Option<Vec<Self>> {
        match self {
            Self::List(items) => Some(items.clone()),
            Self::Strings(Some(items)) => Some(items.iter().cloned().map(Self::String).collect()),
            Self::StringLists(lists) => Some(
                lists
                    .iter()
                    .map(|list| Self::Strings(Some(list.clone())))
                    .collect(),
            ),
            Self::EmptySlice(_) => Some(Vec::new()),
            Self::Nil
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Int64(_)
            | Self::Uint64(_)
            | Self::Float64(_)
            | Self::String(_)
            | Self::Map(_)
            | Self::AnyMap(_)
            | Self::Strings(None) => None,
        }
    }

    /// The value as the `config` crate holds it, with `origin` as its origin. A nil `[]string` is
    /// `config`'s nil.
    #[must_use]
    pub fn to_config_value(&self, origin: Option<&String>) -> Value {
        let kind = match self {
            Self::Nil | Self::Strings(None) => ValueKind::Nil,
            Self::Bool(b) => ValueKind::Boolean(*b),
            Self::Int(i) | Self::Int64(i) => ValueKind::I64(*i),
            Self::Uint64(u) => ValueKind::U64(*u),
            Self::Float64(f) => ValueKind::Float(*f),
            Self::String(s) => ValueKind::String(s.clone()),
            Self::List(items) => {
                ValueKind::Array(items.iter().map(|v| v.to_config_value(origin)).collect())
            }
            Self::Map(map) => ValueKind::Table(
                map.iter()
                    .map(|(k, v)| (k.clone(), v.to_config_value(origin)))
                    .collect(),
            ),
            Self::AnyMap(entries) => ValueKind::Table(
                entries
                    .iter()
                    .map(|(k, v)| (cast::to_string(k), v.to_config_value(origin)))
                    .collect(),
            ),
            Self::Strings(Some(items)) => ValueKind::Array(
                items
                    .iter()
                    .map(|s| Value::new(origin, ValueKind::String(s.clone())))
                    .collect(),
            ),
            Self::StringLists(lists) => ValueKind::Array(
                lists
                    .iter()
                    .map(|l| Self::Strings(Some(l.clone())).to_config_value(origin))
                    .collect(),
            ),
            Self::EmptySlice(_) => ValueKind::Array(Vec::new()),
        };
        Value::new(origin, kind)
    }

    /// The value in the compact form of the Go goldens (`testdata/go/encode.go`'s `dyn`): an
    /// `int` is an integer, a float a JSON float (NaN and the infinities `{"$float64": ...}`), an
    /// `int64` `{"$int64": n}`, a `uint64` `{"$uint64": n}`, a nil `[]string` null, and a
    /// `map[interface{}]interface{}` `{"$imap": [[key, value], ...]}` in the order of the keys'
    /// text.
    #[must_use]
    pub fn compact(&self) -> Json {
        match self {
            Self::Nil | Self::Strings(None) => Json::Null,
            Self::Bool(b) => Json::Bool(*b),
            Self::Int(i) => Json::from(*i),
            Self::Int64(i) => tagged("$int64", Json::from(*i)),
            Self::Uint64(u) => tagged("$uint64", Json::from(*u)),
            Self::Float64(f) => compact_float(*f),
            Self::String(s) => Json::String(s.clone()),
            Self::List(items) => Json::Array(items.iter().map(Self::compact).collect()),
            Self::Map(map) => Json::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), v.compact()))
                    .collect::<JsonMap<_, _>>(),
            ),
            Self::AnyMap(entries) => {
                let mut pairs: Vec<(String, Json)> = entries
                    .iter()
                    .map(|(k, v)| (k.to_string(), Json::Array(vec![k.compact(), v.compact()])))
                    .collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                tagged(
                    "$imap",
                    Json::Array(pairs.into_iter().map(|(_, pair)| pair).collect()),
                )
            }
            Self::Strings(Some(items)) => {
                Json::Array(items.iter().cloned().map(Json::String).collect())
            }
            Self::StringLists(lists) => Json::Array(
                lists
                    .iter()
                    .map(|l| Json::Array(l.iter().cloned().map(Json::String).collect()))
                    .collect(),
            ),
            Self::EmptySlice(_) => Json::Array(Vec::new()),
        }
    }
}

impl fmt::Display for GoValue {
    /// The value laid out as Go's `%v` lays it out (`<nil>`, `[a b]`, `map[a:1 b:2]`), with
    /// floats in std's shortest spelling where Go uses `%g` (`1000000` for Go's `1e+06`) and the
    /// keys of a `map[interface{}]interface{}` in the order of their text (ledger:
    /// `config.error-texts`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("<nil>"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) | Self::Int64(i) => write!(f, "{i}"),
            Self::Uint64(u) => write!(f, "{u}"),
            Self::Float64(x) => f.write_str(&float_text(*x)),
            Self::String(s) => f.write_str(s),
            Self::List(items) => write_list(f, items),
            Self::Strings(items) => write_list(f, items.iter().flatten()),
            Self::StringLists(lists) => {
                f.write_str("[")?;
                for (i, list) in lists.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write_list(f, list)?;
                }
                f.write_str("]")
            }
            Self::EmptySlice(_) => f.write_str("[]"),
            Self::Map(map) => write_map(f, map.iter().map(|(k, v)| (k.clone(), v))),
            Self::AnyMap(entries) => {
                let mut pairs: Vec<(String, &Self)> =
                    entries.iter().map(|(k, v)| (k.to_string(), v)).collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                write_map(f, pairs)
            }
        }
    }
}

/// Go's `%v` of a slice: `[` and the elements, space-separated, then `]`.
fn write_list<I>(f: &mut fmt::Formatter<'_>, items: I) -> fmt::Result
where
    I: IntoIterator,
    I::Item: fmt::Display,
{
    f.write_str("[")?;
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{item}")?;
    }
    f.write_str("]")
}

/// Go's `%v` of a map: `map[`, then `key:value` pairs in the order given, then `]`.
fn write_map<'a, I>(f: &mut fmt::Formatter<'_>, pairs: I) -> fmt::Result
where
    I: IntoIterator<Item = (String, &'a GoValue)>,
{
    f.write_str("map[")?;
    for (i, (key, value)) in pairs.into_iter().enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{key}:{value}")?;
    }
    f.write_str("]")
}

/// `{"<tag>": value}`.
pub(super) fn tagged(tag: &str, value: Json) -> Json {
    let mut map = JsonMap::new();
    drop(map.insert(tag.to_owned(), value));
    Json::Object(map)
}

/// A float in the goldens' compact form (`encode.go`'s `floatValue`).
pub(super) fn compact_float(f: f64) -> Json {
    if f.is_nan() {
        return tagged("$float64", Json::from("NaN"));
    }
    if f.is_infinite() {
        let text = if f > 0.0 { "+Inf" } else { "-Inf" };
        return tagged("$float64", Json::from(text));
    }
    Number::from_f64(f).map_or(Json::Null, Json::Number)
}
