//! `jsonutil.StringInt`, `jsonutil.IntString` and `jsonutil.ParseIntoString`.

use super::gjson::{Kind, parse};
use crate::{
    custom::{GoUnmarshaler, jsoniter_unmarshal, std_unmarshal, std_unmarshal_literal},
    encode::{Encoder, GoEncode},
    error::JsonError,
    gostring::GoString,
    gotype::{GoKind, GoType},
    jsoniter::{Iter, JsoniterDecode},
    stdjson::{StdDecode, StdDecoder},
};

/// `jsonparser.MalformedValueError`.
const MALFORMED_VALUE: &str =
    "Value looks like Number/Boolean/None, but can't find its end: ',' or '}' symbol";
/// `jsonparser.OverflowIntegerError`.
const OVERFLOW_INTEGER: &str = "Value is number, but overflowed while parsing";

/// `jsonparser.ParseInt`.
fn jsonparser_parse_int(bytes: &[u8]) -> Result<i64, &'static str> {
    // Go: buger/jsonparser bytes.go parseInt
    if bytes.is_empty() {
        return Err(MALFORMED_VALUE);
    }
    let (negative, digits) = match bytes.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, bytes),
    };
    let mut n: u64 = 0;
    for &c in digits {
        if !c.is_ascii_digit() {
            return Err(MALFORMED_VALUE);
        }
        if n > u64::MAX / 10 {
            return Err(OVERFLOW_INTEGER);
        }
        n *= 10;
        let Some(next) = n.checked_add(u64::from(c - b'0')) else {
            return Err(OVERFLOW_INTEGER);
        };
        n = next;
    }
    if n > i64::MAX.unsigned_abs() {
        if negative && n == i64::MIN.unsigned_abs() {
            return Ok(i64::MIN);
        }
        return Err(OVERFLOW_INTEGER);
    }
    let magnitude = i64::try_from(n).unwrap_or(i64::MAX);
    Ok(if negative { -magnitude } else { magnitude })
}

/// `jsonutil.StringInt` (`type StringInt int`): an integer that may arrive as a JSON number or
/// a quoted number. It encodes as a plain integer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StringInt(pub isize);

impl GoUnmarshaler for StringInt {
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
        // Go: util/jsonutil/stringInt.go (*StringInt).UnmarshalJSON
        let mut b = data;
        if b.first() == Some(&b'"') {
            // (Go panics on a lone quote; it is treated as empty here.)
            b = b.get(1..b.len().saturating_sub(1)).unwrap_or_default();
        }
        if b.is_empty() {
            return Ok(());
        }
        let i = jsonparser_parse_int(b).map_err(JsonError::msg)?;
        self.0 = isize::try_from(i).unwrap_or_default();
        Ok(())
    }
}

impl GoType for StringInt {
    const KIND: GoKind = GoKind::Int;

    fn go_type_name(out: &mut String) {
        out.push_str("jsonutil.StringInt");
    }
}

impl GoEncode for StringInt {
    fn go_encode(&self, enc: &mut Encoder) {
        self.0.go_encode(enc);
    }

    fn go_is_empty(&self) -> bool {
        self.0 == 0
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        self.0.go_encode_quoted(enc);
    }
}

impl JsoniterDecode for StringInt {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        jsoniter_unmarshal(self, it);
    }
}

impl StdDecode for StringInt {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_unmarshal(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        _from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_unmarshal_literal::<N, Self>(self, item, d)
    }
}

/// `jsonutil.IntString` (`type IntString string`): a string that may arrive as a JSON number
/// (kept as written for integers) or a string; anything else is `invalid type`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntString(pub GoString);

impl GoUnmarshaler for IntString {
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
        // Go: util/jsonutil/intstring.go (*IntString).UnmarshalJSON
        let res = parse(data);
        if res.kind != Kind::Number && res.kind != Kind::String {
            return Err(JsonError::msg("invalid type"));
        }
        self.0 = GoString::from(res.string());
        Ok(())
    }
}

impl GoType for IntString {
    const KIND: GoKind = GoKind::String;

    fn go_type_name(out: &mut String) {
        out.push_str("jsonutil.IntString");
    }
}

impl GoEncode for IntString {
    fn go_encode(&self, enc: &mut Encoder) {
        self.0.go_encode(enc);
    }

    fn go_is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        self.0.go_encode_quoted(enc);
    }
}

impl JsoniterDecode for IntString {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        jsoniter_unmarshal(self, it);
    }
}

impl StdDecode for IntString {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_unmarshal(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        _from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_unmarshal_literal::<N, Self>(self, item, d)
    }
}

/// `jsonutil.ParseIntoString(b, &s)`: sets `*s` to gjson's `String()` of the value unless it is
/// missing or `null` (a `*string` that is not nil is required in Go; `&mut Option` always is).
///
/// # Errors
///
/// Never fails (Go's only error is for a nil `ppString`); the `Result` mirrors the Go
/// signature.
pub fn parse_into_string(b: &[u8], out: &mut Option<GoString>) -> Result<(), JsonError> {
    // Go: util/jsonutil/forcestring.go ParseIntoString
    let result = parse(b);
    if result.exists() && result.raw != b"null" {
        *out = Some(GoString::from(result.string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{IntString, StringInt, parse_into_string};
    use crate::{custom::GoUnmarshaler as _, gostring::GoString};

    /// `Ok` when the values are equal, otherwise an error naming the case.
    macro_rules! check {
        ($got:expr, $want:expr, $what:expr $(,)?) => {{
            let (got, want) = ($got, $want);
            if got == want {
                Ok(())
            } else {
                Err(format!("{}: got {got:?}, want {want:?}", $what))
            }
        }};
    }

    fn error_text<T>(result: Result<T, crate::JsonError>) -> Option<String> {
        result.err().map(|e| e.to_string())
    }

    #[test]
    fn string_int() -> Result<(), String> {
        // Go: util/jsonutil/stringInt_test.go TestStringIntUnmarshalJSON (method level; the
        // struct-level cases are in tests/jsonutil.rs)
        let mut v = StringInt::default();
        check!(error_text(v.unmarshal_json(b"30")), None, "number")?;
        check!(v.0, 30, "number value")?;
        check!(error_text(v.unmarshal_json(b"\"31\"")), None, "quoted")?;
        check!(v.0, 31, "quoted value")?;
        check!(error_text(v.unmarshal_json(b"\"\"")), None, "empty")?;
        check!(v.0, 31, "empty keeps the value")?;
        check!(
            error_text(v.unmarshal_json(b"true")),
            Some(
                "Value looks like Number/Boolean/None, but can't find its end: ',' or '}' symbol"
                    .to_owned(),
            ),
            "malformed",
        )?;
        check!(
            error_text(v.unmarshal_json(b"99999999999999999999")),
            Some("Value is number, but overflowed while parsing".to_owned()),
            "overflow",
        )
    }

    #[test]
    fn int_string() -> Result<(), String> {
        // Go: util/jsonutil/intstring_test.go TestIntStringUnmarshalJSON (method level)
        let mut v = IntString::default();
        check!(error_text(v.unmarshal_json(b"\"30\"")), None, "string")?;
        check!(v.0.as_bytes(), b"30", "string value")?;
        check!(error_text(v.unmarshal_json(b"30")), None, "int")?;
        check!(v.0.as_bytes(), b"30", "int value")?;
        check!(error_text(v.unmarshal_json(b"1.50")), None, "float")?;
        check!(v.0.as_bytes(), b"1.5", "float value")?;
        check!(
            error_text(v.unmarshal_json(b"null")),
            Some("invalid type".to_owned()),
            "null",
        )?;
        check!(
            error_text(v.unmarshal_json(b"[]")),
            Some("invalid type".to_owned()),
            "array",
        )
    }

    #[test]
    fn parse_into_string_cases() -> Result<(), String> {
        // Go: util/jsonutil/forcestring_test.go Test_ParseIntoString
        let cases: [(&str, &[u8], Option<&str>); 6] = [
            ("empty", b"", None),
            ("quoted_1", b"\"1\"", Some("1")),
            ("unquoted_1", b"1", Some("1")),
            ("null", b"null", None),
            ("quoted_null", b"\"null\"", Some("null")),
            ("quoted_hello", b"\"hello\"", Some("hello")),
        ];
        for (name, input, want) in cases {
            let mut got = None;
            check!(error_text(parse_into_string(input, &mut got)), None, name)?;
            check!(got, want.map(GoString::from), name)?;
        }
        Ok(())
    }
}
