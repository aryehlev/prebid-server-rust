//! Prebid's lenient readers: `jsonutil.StringInt`, `jsonutil.IntString` and
//! `jsonutil.ParseIntoString`.

use pbs_errortypes::GoError;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{Error as _, IgnoredAny},
};
use serde_json::value::RawValue;

use crate::replace_lone_surrogates;

/// Go: `jsonutil.StringInt`: an integer a request may send as a number or as a string holding
/// one (`"30"` and `30` both read 30). It is written as a number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StringInt(pub i64);

impl StringInt {
    /// Go: `(*StringInt).UnmarshalJSON(raw)`: reads the JSON value `raw` as it is written. The
    /// quotes of a string are dropped without unescaping what is between them; nothing (the
    /// empty string) leaves the value as it was; anything else must be a base-10 integer.
    ///
    /// Go parses with `jsonparser.ParseInt` and this with `i64::from_str`, which differ on two
    /// spellings: `"+5"` reads 5 here and is an error in Go, and `"-"` is an error here and reads
    /// 0 in Go (`json.string-int-parse`).
    ///
    /// # Errors
    ///
    /// A generic error (Go: `*errors.errorString`) when `raw` is not an integer, null and
    /// booleans included.
    pub fn set_from_json(&mut self, raw: &str) -> Result<(), GoError> {
        let digits = raw
            .strip_prefix('"')
            .map_or(raw, |inner| inner.strip_suffix('"').unwrap_or(inner));
        if digits.is_empty() {
            return Ok(());
        }
        self.0 = digits
            .parse()
            .map_err(|e| GoError::generic(format!("{e}: {raw}")))?;
        Ok(())
    }
}

impl<'de> Deserialize<'de> for StringInt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let mut value = Self::default();
        value.set_from_json(raw.get()).map_err(D::Error::custom)?;
        Ok(value)
    }
}

/// Go: `jsonutil.IntString`: a string a request may send as a number or as a string (`30` and
/// `"30"` both read `"30"`). It is written as a string.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct IntString(pub String);

impl IntString {
    /// Go: `(*IntString).UnmarshalJSON(raw)`: a string reads as its text and a number as gjson's
    /// `String()` gives it: an integer as it is written, any other number in the shortest form
    /// that reads back to the same `f64`, without an exponent (`1.50` and `15e-1` read `"1.5"`),
    /// and a number beyond `f64`'s range as `+Inf` or `-Inf`.
    ///
    /// jsoniter fails the document on such a number before the method sees it, and `serde_json`
    /// hands it over (`json.out-of-range-numbers`). A lone surrogate escape in a string is read
    /// as jsoniter reads it, where gjson pairs it with the escape after it
    /// (`json.surrogate-pairing`).
    ///
    /// # Errors
    ///
    /// `invalid type` (Go: `*errors.errorString`) for null, booleans, arrays and objects, and a
    /// generic error for bytes that are not a JSON string or number.
    pub fn set_from_json(&mut self, raw: &str) -> Result<(), GoError> {
        match raw.as_bytes().first() {
            Some(b'"') => {
                self.0 = serde_json::from_slice(&replace_lone_surrogates(raw.as_bytes()))
                    .map_err(|e| GoError::generic(e.to_string()))?;
                Ok(())
            }
            Some(b'-' | b'0'..=b'9') => {
                self.0 = number_text(raw)?;
                Ok(())
            }
            _ => Err(GoError::generic("invalid type")),
        }
    }
}

impl<'de> Deserialize<'de> for IntString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let mut value = Self::default();
        value.set_from_json(raw.get()).map_err(D::Error::custom)?;
        Ok(value)
    }
}

/// gjson's `Result.String()` for the JSON number `raw`: its text when it is an optional minus
/// followed by digits only, else `strconv.FormatFloat(n, 'f', -1, 64)` of the nearest `f64`,
/// which Rust's `f64` display matches for a finite value; beyond `f64`'s range `strconv` gives
/// `+Inf` or `-Inf` (gjson drops `ParseFloat`'s range error).
fn number_text(raw: &str) -> Result<String, GoError> {
    // The syntax is checked without a range check: serde_json skips a number without parsing it.
    let _number: IgnoredAny =
        serde_json::from_str(raw).map_err(|e| GoError::generic(e.to_string()))?;
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(raw.to_owned());
    }
    let number: f64 = raw
        .parse()
        .map_err(|e| GoError::generic(format!("{e}: {raw}")))?;
    Ok(if number.is_infinite() {
        if number.is_sign_positive() {
            "+Inf".to_owned()
        } else {
            "-Inf".to_owned()
        }
    } else {
        number.to_string()
    })
}

/// Go: `jsonutil.ParseIntoString(data, ppString)`: sets `*target` to the string gjson reads.
///
/// `*target` is left as it was when `data` holds nothing or null. A string gives its text, a
/// number its text as [`IntString`] gives it (`+Inf` and `-Inf` beyond `f64`'s range), a boolean
/// `true` or `false`, and an object or array the bytes from its first bracket to the end of
/// `data`, as written.
///
/// gjson also reads values out of bytes that are not JSON (an unterminated string, a number
/// followed by other bytes, `nan`, a literal cut short); those leave `*target` unset here
/// (`json.parse-into-string-lenient`). A lone surrogate escape is read as jsoniter reads it,
/// where gjson pairs it with the escape after it (`json.surrogate-pairing`).
///
/// # Errors
///
/// `ppString is nil` when `target` is `None` (Go: a nil `ppString`).
pub fn parse_into_string(data: &[u8], target: Option<&mut Option<String>>) -> Result<(), GoError> {
    let Some(target) = target else {
        return Err(GoError::generic("ppString is nil"));
    };
    if let Some(text) = read_string(data) {
        *target = Some(text);
    }
    Ok(())
}

/// The string `parse_into_string` reads from `data`, if any.
fn read_string(data: &[u8]) -> Option<String> {
    // gjson skips every byte up to and including the space before the value.
    let start = data.iter().position(|&b| b > b' ')?;
    let rest = data.get(start..)?;
    if matches!(rest.first(), Some(b'{' | b'[')) {
        return Some(String::from_utf8_lossy(rest).into_owned());
    }
    let text = replace_lone_surrogates(rest);
    // A RawValue checks the syntax only, so a number beyond f64's range reads as gjson reads it.
    let raw: &RawValue = serde_json::from_slice(&text).ok()?;
    let raw = raw.get();
    match raw.as_bytes().first()? {
        b'"' => serde_json::from_str(raw).ok(),
        b't' => Some("true".to_owned()),
        b'f' => Some("false".to_owned()),
        b'-' | b'0'..=b'9' => number_text(raw).ok(),
        _ => None,
    }
}
