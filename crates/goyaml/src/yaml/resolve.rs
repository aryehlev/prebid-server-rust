//! Plain scalar resolution (Go: `resolve.go`): which YAML type a scalar is, and its value.

use std::borrow::Cow;

use pbs_gocompat::{math, strconv, time};

use super::UnmarshalError;
use crate::int_to_float;

/// Go: `nullTag`.
pub(super) const NULL_TAG: &[u8] = b"!!null";
/// Go: `boolTag`.
pub(super) const BOOL_TAG: &[u8] = b"!!bool";
/// Go: `strTag`.
pub(super) const STR_TAG: &[u8] = b"!!str";
/// Go: `intTag`.
pub(super) const INT_TAG: &[u8] = b"!!int";
/// Go: `floatTag`.
pub(super) const FLOAT_TAG: &[u8] = b"!!float";
/// Go: `timestampTag`.
pub(super) const TIMESTAMP_TAG: &[u8] = b"!!timestamp";
/// Go: `seqTag`.
pub(super) const SEQ_TAG: &[u8] = b"!!seq";
/// Go: `mapTag`.
pub(super) const MAP_TAG: &[u8] = b"!!map";
/// Go: `binaryTag`.
pub(super) const BINARY_TAG: &[u8] = b"!!binary";
/// Go: `mergeTag`.
pub(super) const MERGE_TAG: &[u8] = b"!!merge";

/// Go: `longTagPrefix`.
const LONG_TAG_PREFIX: &[u8] = b"tag:yaml.org,2002:";

/// The value a scalar resolves to: the dynamic type of Go's `interface{}` result.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Resolved<'a> {
    /// `nil` (`!!null`).
    Null,
    /// Go `bool`.
    Bool(bool),
    /// Go `int` (64 bits; `resolve` never yields `int64` on 64-bit platforms).
    Int(i64),
    /// Go `uint64`, for integers above `math.MaxInt64`.
    Uint(u64),
    /// Go `float64`.
    Float(f64),
    /// Go `string`.
    Str(&'a str),
    /// Go `time.Time`.
    Time(time::Time),
    /// The decoded bytes of a `!!binary` scalar (Go: `string(data)`).
    Binary(Vec<u8>),
}

/// Go: `shortTag`.
pub(super) fn short_tag(tag: &[u8]) -> Cow<'_, [u8]> {
    let Some(rest) = tag.strip_prefix(LONG_TAG_PREFIX) else {
        return Cow::Borrowed(tag);
    };
    let known: &[&'static [u8]] = &[
        NULL_TAG,
        BOOL_TAG,
        STR_TAG,
        INT_TAG,
        FLOAT_TAG,
        TIMESTAMP_TAG,
        SEQ_TAG,
        MAP_TAG,
        BINARY_TAG,
        MERGE_TAG,
    ];
    if let Some(stag) = known.iter().find(|s| s.get(2..) == Some(rest)) {
        return Cow::Borrowed(stag);
    }
    let mut out = b"!!".to_vec();
    out.extend_from_slice(rest);
    Cow::Owned(out)
}

/// Go: `resolvableTag`.
fn resolvable_tag(tag: &[u8]) -> bool {
    tag.is_empty()
        || tag == STR_TAG
        || tag == BOOL_TAG
        || tag == INT_TAG
        || tag == FLOAT_TAG
        || tag == NULL_TAG
        || tag == TIMESTAMP_TAG
}

/// Go: `resolveTable`: the hint for the first byte of a plain scalar.
const fn hint(c: u8) -> u8 {
    match c {
        b'+' | b'-' => b'S',
        b'0'..=b'9' => b'D',
        b'y' | b'Y' | b'n' | b'N' | b't' | b'T' | b'f' | b'F' | b'o' | b'O' | b'~' => b'M',
        b'.' => b'.',
        _ => 0,
    }
}

/// Go: `resolveMap`.
fn resolve_map(input: &str) -> Option<(&'static [u8], Resolved<'static>)> {
    Some(match input {
        "true" | "True" | "TRUE" => (BOOL_TAG, Resolved::Bool(true)),
        "false" | "False" | "FALSE" => (BOOL_TAG, Resolved::Bool(false)),
        "" | "~" | "null" | "Null" | "NULL" => (NULL_TAG, Resolved::Null),
        ".nan" | ".NaN" | ".NAN" => (FLOAT_TAG, Resolved::Float(math::NAN)),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            (FLOAT_TAG, Resolved::Float(f64::INFINITY))
        }
        "-.inf" | "-.Inf" | "-.INF" => (FLOAT_TAG, Resolved::Float(f64::NEG_INFINITY)),
        // Go also maps "<<" to the merge tag, but no hint leads to it ('<' has none).
        _ => return None,
    })
}

/// Go: `yamlStyleFloat`, `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`.
fn yaml_style_float(s: &str) -> bool {
    let s = s.as_bytes();
    let digits = |from: usize| {
        s.get(from..)
            .map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count())
    };
    let mut i = usize::from(matches!(s.first(), Some(b'-' | b'+')));
    if s.get(i) == Some(&b'.') {
        let n = digits(i + 1);
        if n == 0 {
            return false;
        }
        i += 1 + n;
    } else {
        let n = digits(i);
        if n == 0 {
            return false;
        }
        i += n;
        if s.get(i) == Some(&b'.') {
            i += 1;
            i += digits(i);
        }
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some(b'-' | b'+')) {
            j += 1;
        }
        let n = digits(j);
        if n == 0 {
            return false;
        }
        i = j + n;
    }
    i == s.len()
}

/// Go: `allowedTimestampFormats`.
const ALLOWED_TIMESTAMP_FORMATS: [&str; 4] = [
    "2006-1-2T15:4:5.999999999Z07:00", // RCF3339Nano with short date fields.
    "2006-1-2t15:4:5.999999999Z07:00", // RFC3339Nano with short date fields and lower-case "t".
    "2006-1-2 15:4:5.999999999",       // space separated with no time zone
    "2006-1-2",                        // date only
];

/// Go: `parseTimestamp`.
fn parse_timestamp(s: &str) -> Option<time::Time> {
    // Quick check: all date formats start with YYYY-.
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits != 4 || s.as_bytes().get(digits) != Some(&b'-') {
        return None;
    }
    ALLOWED_TIMESTAMP_FORMATS
        .iter()
        .find_map(|format| time::parse(format, s).ok())
}

/// Go's integer result of `strconv.ParseInt(s, base, 64)`: always `int` on 64-bit platforms.
fn parse_int(s: &str, base: i64) -> Option<Resolved<'static>> {
    strconv::parse_int(s, base, 64).ok().map(Resolved::Int)
}

/// Go: `strconv.ParseUint(s, base, 64)` as a `uint64` result.
fn parse_uint(s: &str, base: i64) -> Option<Resolved<'static>> {
    strconv::parse_uint(s, base, 64).ok().map(Resolved::Uint)
}

/// Go: `resolve(tag, in)`: the resolved tag and value of a scalar, failing (Go: `failf`) when an
/// explicit tag does not match the value.
pub(super) fn resolve<'a>(
    tag: &'a [u8],
    input: &'a str,
) -> Result<(Cow<'a, [u8]>, Resolved<'a>), UnmarshalError> {
    let tag = short_tag(tag);
    if !resolvable_tag(&tag) {
        return Ok((tag, Resolved::Str(input)));
    }

    let (rtag, out): (Cow<'a, [u8]>, Resolved<'a>) = resolve_value(&tag, input);

    // Go: the deferred check.
    if tag.is_empty() || *tag == *rtag || *tag == *STR_TAG || *tag == *BINARY_TAG {
        return Ok((rtag, out));
    }
    if *tag == *FLOAT_TAG
        && *rtag == *INT_TAG
        && let Resolved::Int(v) = out
    {
        return Ok((Cow::Borrowed(FLOAT_TAG), Resolved::Float(int_to_float(v))));
    }
    let mut msg = b"yaml: cannot decode ".to_vec();
    msg.extend_from_slice(&short_tag(&rtag));
    msg.extend_from_slice(b" `");
    msg.extend_from_slice(input.as_bytes());
    msg.extend_from_slice(b"` as a ");
    msg.extend_from_slice(&short_tag(&tag));
    Err(UnmarshalError::Fatal(msg))
}

/// The body of Go's `resolve` before its deferred check.
fn resolve_value<'a>(tag: &[u8], input: &'a str) -> (Cow<'a, [u8]>, Resolved<'a>) {
    // Any data is accepted as a !!str or !!binary.
    // Otherwise, the prefix is enough of a hint about what it might be.
    let hint = input.as_bytes().first().map_or(b'N', |&c| hint(c));
    if hint != 0 && tag != STR_TAG && tag != BINARY_TAG {
        // Handle things we can lookup in a map.
        if let Some((rtag, value)) = resolve_map(input) {
            return (Cow::Borrowed(rtag), value);
        }

        // Base 60 floats are a bad idea, were dropped in YAML 1.2, and
        // are purposefully unsupported here. They're still quoted on
        // the way out for compatibility with other parser, though.

        match hint {
            b'.' => {
                // Not in the map, so maybe a normal float.
                if let Ok(floatv) = strconv::parse_float(input, 64) {
                    return (Cow::Borrowed(FLOAT_TAG), Resolved::Float(floatv));
                }
            }
            b'D' | b'S' => {
                // Int, float, or timestamp.
                // Only try values as a timestamp if the value is unquoted or there's an explicit
                // !!timestamp tag.
                if (tag.is_empty() || tag == TIMESTAMP_TAG)
                    && let Some(t) = parse_timestamp(input)
                {
                    return (Cow::Borrowed(TIMESTAMP_TAG), Resolved::Time(t));
                }

                let plain = input.replace('_', "");
                if let Some(v) = parse_int(&plain, 0).or_else(|| parse_uint(&plain, 0)) {
                    return (Cow::Borrowed(INT_TAG), v);
                }
                if yaml_style_float(&plain)
                    && let Ok(floatv) = strconv::parse_float(&plain, 64)
                {
                    return (Cow::Borrowed(FLOAT_TAG), Resolved::Float(floatv));
                }
                if let Some(rest) = plain.strip_prefix("0b") {
                    if let Some(v) = parse_int(rest, 2).or_else(|| parse_uint(rest, 2)) {
                        return (Cow::Borrowed(INT_TAG), v);
                    }
                } else if let Some(rest) = plain.strip_prefix("-0b")
                    && let Some(v) = parse_int(&format!("-{rest}"), 2)
                {
                    return (Cow::Borrowed(INT_TAG), v);
                }
                // Octals as introduced in version 1.2 of the spec.
                // Octals from the 1.1 spec, spelled as 0777, are still
                // decoded by default in v3 as well for compatibility.
                // May be dropped in v4 depending on how usage evolves.
                if let Some(rest) = plain.strip_prefix("0o") {
                    if let Some(v) = parse_int(rest, 8).or_else(|| parse_uint(rest, 8)) {
                        return (Cow::Borrowed(INT_TAG), v);
                    }
                } else if let Some(rest) = plain.strip_prefix("-0o")
                    && let Some(v) = parse_int(&format!("-{rest}"), 8)
                {
                    return (Cow::Borrowed(INT_TAG), v);
                }
            }
            // 'M' (and 'N'): we've already checked the map above.
            _ => {}
        }
    }
    (Cow::Borrowed(STR_TAG), Resolved::Str(input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_style_float_matches_the_go_regexp() {
        for (s, want) in [
            ("1", true),
            ("-1.5", true),
            ("+.5", true),
            (".5e10", true),
            ("1.", true),
            ("1.e5", true),
            ("1e", false),
            (".", false),
            ("-", false),
            ("1e+", false),
            ("08", true),
            ("1_0", false),
            ("0x10", false),
        ] {
            assert_eq!(yaml_style_float(s), want, "{s}");
        }
    }

    #[test]
    fn short_tag_maps_the_long_prefix() {
        assert_eq!(&*short_tag(b"tag:yaml.org,2002:str"), b"!!str", "known");
        assert_eq!(&*short_tag(b"tag:yaml.org,2002:foo"), b"!!foo", "unknown");
        assert_eq!(&*short_tag(b"!local"), b"!local", "local");
    }
}
