//! Go viper v1.12.0 `util.go`: case-insensitive maps, nested map writes and size parsing.

use std::{collections::BTreeMap, mem};

use pbs_gocompat::strings::{to_lower, trim_space};
use pbs_goyaml::{Type, Value};

use crate::cast;

/// A `map[string]interface{}`, the shape of viper's registries.
pub(super) type StringMap = BTreeMap<String, Value>;

/// Go: `toCaseInsensitiveValue`: a copy of a map value with its keys (and those of its nested
/// maps) lowercased; other values as they are.
pub(super) fn to_case_insensitive_value(value: Value) -> Value {
    match value {
        Value::Map(Type::Interface, Type::Interface, _) => {
            let m = cast::to_string_map(&value).unwrap_or_default();
            Value::StringMap(&Type::Interface, Some(copy_and_insensitivise_map(&m)))
        }
        Value::StringMap(Type::Interface, entries) => Value::StringMap(
            &Type::Interface,
            Some(copy_and_insensitivise_map(&entries.unwrap_or_default())),
        ),
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
        | Value::Map(..) => value,
    }
}

/// Go: `copyAndInsensitiviseMap`: a copy with lowercased keys, recursing into nested
/// `map[string]interface{}` and `map[interface{}]interface{}` values (which become the former).
///
/// Of keys that lowercase alike, the last in byte order wins (Go: whichever its random map
/// iteration reaches last).
pub(super) fn copy_and_insensitivise_map(m: &StringMap) -> StringMap {
    let mut nm = BTreeMap::new();
    for (key, val) in m {
        let value = match val {
            Value::Map(Type::Interface, Type::Interface, _) => Value::StringMap(
                &Type::Interface,
                Some(copy_and_insensitivise_map(
                    &cast::to_string_map(val).unwrap_or_default(),
                )),
            ),
            Value::StringMap(Type::Interface, entries) => Value::StringMap(
                &Type::Interface,
                Some(copy_and_insensitivise_map(
                    entries.as_ref().unwrap_or(&BTreeMap::new()),
                )),
            ),
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
            | Value::Map(..) => val.clone(),
        };
        let _previous = nm.insert(to_lower(key), value);
    }
    nm
}

/// Go: `insensitiviseMap`: lowercases the keys of `m` and of its nested maps in place, turning
/// nested `map[interface{}]interface{}` values into `map[string]interface{}` (maps inside slices
/// are left alone).
///
/// When keys lowercase alike, a key that was not already lowercase wins over the lowercase one,
/// as in Go (which deletes the original key and writes its value under the lowercase one); of
/// several such keys, the last in byte order wins (Go: the last its random iteration reaches).
pub(super) fn insensitivise_map(m: &mut StringMap) {
    for (key, mut val) in mem::take(m) {
        insensitivise_value(&mut val);
        let lower = to_lower(&key);
        if lower == key {
            let _kept = m.entry(lower).or_insert(val);
        } else {
            let _previous = m.insert(lower, val);
        }
    }
}

/// The nested-map part of [`insensitivise_map`] for one value.
fn insensitivise_value(val: &mut Value) {
    if let Value::Map(Type::Interface, Type::Interface, _) = val {
        // Go: `val = cast.ToStringMap(val)`.
        *val = Value::StringMap(&Type::Interface, cast::to_string_map(val));
    }
    if let Value::StringMap(Type::Interface, Some(nested)) = val {
        insensitivise_map(nested);
    }
}

/// Go: `deepSearch(m, parents)[last] = value`: sets `value` at the path, creating the
/// intermediate maps and replacing intermediate values that are not `map[string]interface{}`.
pub(super) fn deep_set(m: &mut StringMap, parents: &[&str], last: String, value: Value) {
    let Some((first, rest)) = parents.split_first() else {
        let _previous = m.insert(last, value);
        return;
    };
    let slot = m.entry((*first).to_owned()).or_default();
    if let Value::StringMap(Type::Interface, Some(inner)) = slot {
        deep_set(inner, rest, last, value);
        return;
    }
    // Go: "intermediate key does not exist / is a value => create it / replace with a new map".
    let mut inner = BTreeMap::new();
    deep_set(&mut inner, rest, last, value);
    *slot = Value::StringMap(&Type::Interface, Some(inner));
}

/// Go: `safeMul`: `a * b`, or 0 if it overflows.
const fn safe_mul(a: u64, b: u64) -> u64 {
    let c = a.wrapping_mul(b);
    if a > 1 && b > 1 && c / b != a {
        return 0;
    }
    c
}

/// Go: `parseSizeInBytes`: a size such as `"1 kb"`, `"12mb"` or `"3Gb"` in bytes (binary
/// multiples); a `b` without a known unit before it is dropped, and anything `cast.ToInt` cannot
/// read is 0.
pub(super) fn parse_size_in_bytes(input: &str) -> u64 {
    let mut size_str = trim_space(input);
    let bytes = size_str.as_bytes();
    let mut multiplier: u64 = 1;
    if let Some(last) = bytes.len().checked_sub(1)
        && last > 1
        && matches!(bytes.get(last), Some(b'b' | b'B'))
    {
        // Go: unicode.ToLower(rune(sizeStr[lastChar-1])), which is k, m or g only for ASCII.
        let cut = match bytes.get(last - 1).map(u8::to_ascii_lowercase) {
            Some(b'k') => {
                multiplier = 1 << 10;
                last - 1
            }
            Some(b'm') => {
                multiplier = 1 << 20;
                last - 1
            }
            Some(b'g') => {
                multiplier = 1 << 30;
                last - 1
            }
            Some(_) | None => last,
        };
        size_str = trim_space(size_str.get(..cut).unwrap_or(size_str));
    }
    let size = cast::to_int(&Value::String(size_str.to_owned()));
    safe_mul(u64::try_from(size).unwrap_or(0), multiplier)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pbs_goyaml::{Type, Value};

    use super::{copy_and_insensitivise_map, insensitivise_map, parse_size_in_bytes};

    fn string_map(entries: Vec<(&str, Value)>) -> Value {
        Value::StringMap(
            &Type::Interface,
            Some(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect(),
            ),
        )
    }

    /// Go: viper's `TestCopyAndInsensitiviseMap`.
    #[test]
    fn copy_and_insensitivise() -> Result<(), String> {
        let given: BTreeMap<String, Value> = [
            ("Foo".to_owned(), Value::Int(32)),
            (
                "Bar".to_owned(),
                Value::Map(
                    &Type::Interface,
                    &Type::Interface,
                    Some(vec![
                        (
                            Value::String("ABc".to_owned()),
                            Value::String("A".to_owned()),
                        ),
                        (
                            Value::String("cDE".to_owned()),
                            Value::String("B".to_owned()),
                        ),
                    ]),
                ),
            ),
        ]
        .into_iter()
        .collect();
        let got = copy_and_insensitivise_map(&given);
        let want: BTreeMap<String, Value> = [
            ("foo".to_owned(), Value::Int(32)),
            (
                "bar".to_owned(),
                string_map(vec![
                    ("abc", Value::String("A".to_owned())),
                    ("cde", Value::String("B".to_owned())),
                ]),
            ),
        ]
        .into_iter()
        .collect();
        if got != want {
            return Err(format!("got {got:?}, want {want:?}"));
        }
        // Go: the original map is left alone.
        if !given.contains_key("Foo") {
            return Err("the original was modified".to_owned());
        }
        Ok(())
    }

    #[test]
    fn insensitivise_prefers_the_mixed_case_key() -> Result<(), String> {
        let mut m: BTreeMap<String, Value> = [
            ("foo".to_owned(), Value::Int(1)),
            ("Foo".to_owned(), Value::Int(2)),
            (
                "Nested".to_owned(),
                string_map(vec![("InNer", Value::Bool(true))]),
            ),
        ]
        .into_iter()
        .collect();
        insensitivise_map(&mut m);
        let want: BTreeMap<String, Value> = [
            ("foo".to_owned(), Value::Int(2)),
            (
                "nested".to_owned(),
                string_map(vec![("inner", Value::Bool(true))]),
            ),
        ]
        .into_iter()
        .collect();
        if m != want {
            return Err(format!("got {m:?}, want {want:?}"));
        }
        Ok(())
    }

    /// Go: viper's `TestSizeInBytes`.
    #[test]
    fn size_in_bytes() -> Result<(), String> {
        for (input, want) in [
            ("", 0),
            ("b", 0),
            ("12 bytes", 0),
            ("200000000000gb", 0),
            ("12 b", 12),
            ("43 MB", 43 << 20),
            ("10mb", 10 << 20),
            ("1gb", 1 << 30),
        ] {
            let got = parse_size_in_bytes(input);
            if got != want {
                return Err(format!("parseSizeInBytes({input:?}) = {got}, want {want}"));
            }
        }
        Ok(())
    }
}
