//! [`JsonNumber`]: Go's `json.Number`.

use core::fmt;

use crate::{
    encode::{Encoder, Flavor, GoEncode},
    error::JsonError,
    gostring::GoString,
    gotype::{GoKind, GoType},
    jsoniter::{Iter, JsoniterDecode, ValueType, reread_string},
    stdjson::{Class, StdDecode, StdDecoder, Store, literal_store, std_decode_scalar},
    strconv::{num_error_text, parse_float_go, parse_int_go, quote},
};

/// Go's `json.Number`: the literal text of a JSON number, kept as a string.
///
/// Its JSON behaviour follows Go's, which differs between the libraries:
///
/// * jsoniter (`jsonutil.Unmarshal`) stores a JSON string's contents **unvalidated** (`"aid":
///   "abc"` decodes to `"abc"`), turns `null` into `""`, and otherwise keeps the run of
///   `+-.eE0123456789` bytes at the value (`+1`, `1-2` and `.5` are kept as they are; anything else
///   fails with `readNumberAsString: invalid number`).
/// * `encoding/json` decoding accepts a number literal, or a string holding a valid JSON number,
///   and fails with `json: invalid number literal, trying to unmarshal "\"abc\"" into Number`
///   otherwise; `null` leaves the value unchanged.
/// * `jsonutil.Marshal` writes the text raw (even when it is not a number), and `0` for `""`.
/// * `json.Marshal` writes the text, `0` for `""`, and fails with `json: invalid number literal
///   "abc"` when the text is not a JSON number. Adapters such as adtarget hit exactly this: a
///   string `aid` decodes with jsonutil and the later `json.Marshal` fails.
///
/// `,omitempty` omits `""`; `,string` quotes the text when encoding.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsonNumber(pub GoString);

impl JsonNumber {
    /// A number with the given literal text (no validation, as in Go).
    #[must_use]
    pub fn new<T: Into<GoString>>(text: T) -> Self {
        Self(text.into())
    }

    /// The literal text (Go's `string(n)`).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Go's `n.String()`, with invalid UTF-8 replaced the way Go's `for range` would see it.
    #[must_use]
    pub fn string(&self) -> String {
        self.0.to_string_lossy()
    }

    /// Whether the text is empty (Go's `n == ""`).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Go's `n.Int64()`: `strconv.ParseInt(string(n), 10, 64)`.
    ///
    /// # Errors
    ///
    /// Go's `*strconv.NumError` text, for example `strconv.ParseInt: parsing "1.5": invalid
    /// syntax` or `strconv.ParseInt: parsing "9223372036854775808": value out of range` (Go also
    /// returns the clamped value with a range error; no Prebid Server caller reads it).
    pub fn int64(&self) -> Result<i64, JsonError> {
        parse_int_go(self.as_bytes())
            .map_err(|kind| JsonError::Message(num_error_text("ParseInt", self.as_bytes(), kind)))
    }

    /// Go's `n.Float64()`: `strconv.ParseFloat(string(n), 64)`, which also accepts `Inf`, `NaN`,
    /// `_` digit separators and hexadecimal floats such as `0x1.8p1`.
    ///
    /// # Errors
    ///
    /// Go's `*strconv.NumError` text, for example `strconv.ParseFloat: parsing "abc": invalid
    /// syntax` or `strconv.ParseFloat: parsing "1e400": value out of range` (Go also returns
    /// `±Inf` with a range error; no Prebid Server caller reads it).
    pub fn float64(&self) -> Result<f64, JsonError> {
        match parse_float_go(self.as_bytes(), false) {
            (v, None) => Ok(v),
            (_, Some(kind)) => Err(JsonError::Message(num_error_text(
                "ParseFloat",
                self.as_bytes(),
                kind,
            ))),
        }
    }
}

impl fmt::Debug for JsonNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for JsonNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<&str> for JsonNumber {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for JsonNumber {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<GoString> for JsonNumber {
    fn from(text: GoString) -> Self {
        Self(text)
    }
}

/// Go's `isValidNumber`: whether `s` is a JSON number literal.
fn is_valid_number(s: &[u8]) -> bool {
    // Go: encoding/json/encode.go isValidNumber
    let digit = |i: usize| s.get(i).is_some_and(u8::is_ascii_digit);
    let mut i = usize::from(s.first() == Some(&b'-'));
    match s.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            i += 1;
            while digit(i) {
                i += 1;
            }
        }
        _ => return false,
    }
    if s.get(i) == Some(&b'.') && digit(i + 1) {
        i += 2;
        while digit(i) {
            i += 1;
        }
    }
    if matches!(s.get(i), Some(b'e' | b'E')) && s.len() >= i + 2 {
        i += 1;
        if matches!(s.get(i), Some(b'+' | b'-')) {
            i += 1;
            if i == s.len() {
                return false;
            }
        }
        while digit(i) {
            i += 1;
        }
    }
    i == s.len()
}

impl GoType for JsonNumber {
    const KIND: GoKind = GoKind::String;

    fn go_type_name(out: &mut String) {
        out.push_str("json.Number");
    }
}

/// `encoding/json`'s `stringEncoder` for `json.Number`, inside quotes for `,string`.
fn std_encode_number(n: &JsonNumber, enc: &mut Encoder, quoted: bool) {
    // Go: encoding/json/encode.go stringEncoder (numberType)
    let text: &[u8] = if n.is_empty() { b"0" } else { n.as_bytes() };
    if !is_valid_number(text) {
        let mut err = b"json: invalid number literal ".to_vec();
        err.extend_from_slice(&quote(text));
        enc.set_error(err);
        return;
    }
    if quoted {
        enc.write_byte(b'"');
    }
    enc.write_raw(text);
    if quoted {
        enc.write_byte(b'"');
    }
}

impl GoEncode for JsonNumber {
    fn go_encode(&self, enc: &mut Encoder) {
        match enc.flavor() {
            Flavor::Jsoniter => {
                // Go: json-iterator/go reflect_json_number.go jsonNumberCodec.Encode
                if self.is_empty() {
                    enc.write_byte(b'0');
                } else {
                    enc.write_raw(self.as_bytes());
                }
            }
            Flavor::Std { .. } => std_encode_number(self, enc, false),
        }
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        std_encode_number(self, enc, true);
    }
}

impl JsoniterDecode for JsonNumber {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        // Go: json-iterator/go reflect_json_number.go jsonNumberCodec.Decode
        self.0 = match it.what_is_next() {
            ValueType::String => GoString::from_bytes(it.read_string()),
            ValueType::Nil => {
                it.skip_four_bytes(b'n', b'u', b'l', b'l');
                GoString::new()
            }
            ValueType::Invalid
            | ValueType::Number
            | ValueType::Bool
            | ValueType::Array
            | ValueType::Object => GoString::from_bytes(it.read_number_as_string()),
        };
    }

    fn jsoniter_decode_string_mode(&mut self, it: &mut Iter<'_>) {
        // Go: stringModeStringDecoder (json.Number is a string kind)
        self.jsoniter_decode(it);
        self.0 = GoString::from_bytes(reread_string(self.as_bytes()));
    }
}

impl StdDecode for JsonNumber {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_scalar::<N, Self>(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        // Go: encoding/json/decode.go literalStore (numberType)
        if matches!(item.first(), Some(b'-' | b'0'..=b'9')) {
            // Unchecked, as in Go: a `,string` value such as "12abc" is stored as it is.
            self.0 = GoString::from_bytes(item.to_vec());
            return Ok(());
        }
        if let Store::Str(s) = literal_store::<N>(item, from_quoted, d, Class::String)? {
            if !is_valid_number(&s) {
                let mut err = b"json: invalid number literal, trying to unmarshal ".to_vec();
                err.extend_from_slice(&quote(item));
                err.extend_from_slice(b" into Number");
                return Err(JsonError::Message(err));
            }
            self.0 = GoString::from_bytes(s);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonNumber, is_valid_number};
    use crate::{jsonutil, stdjson};

    macro_rules! check {
        ($got:expr, $want:expr, $what:expr $(,)?) => {{
            let got = $got;
            let want = $want;
            if got == want {
                Ok(())
            } else {
                Err(format!("{}: got {got:?}, want {want:?}", $what))
            }
        }};
    }

    #[test]
    fn valid_numbers() -> Result<(), String> {
        // Go: encoding/json/number_test.go TestNumberIsValid
        for s in [
            "0", "-0", "1", "-1", "0.1", "-0.1", "1234", "-1234", "12.34", "-12.34", "12E0",
            "12E1", "12e34", "12E-0", "12e+1", "12e-34", "-12E0", "-12E1", "-12e34", "-12E-0",
            "-12e+1", "-12e-34", "1.2E0", "1.2E1", "1.2e34", "1.2E-0", "1.2e+1", "1.2e-34",
            "-1.2E0", "-1.2E1", "-1.2e34", "-1.2E-0", "-1.2e+1", "-1.2e-34", "0E0", "0E1", "0e34",
            "0E-0", "0e+1", "0e-34", "-0E0", "-0E1", "-0e34", "-0E-0", "-0e+1", "-0e-34",
        ] {
            check!(is_valid_number(s.as_bytes()), true, s)?;
        }
        for s in [
            "", "invalid", "1.0.1", "1..1", "-1-2", "012a42", "01.2", "012", "12E12.12", "1e2e3",
            "1e+-2", "1e--23", "1e", "e1", "1e+", "1ea", "1a", "1.a", "1.", "01", "1.e1",
        ] {
            check!(is_valid_number(s.as_bytes()), false, s)?;
        }
        Ok(())
    }

    #[test]
    fn methods() -> Result<(), String> {
        let text = |r: Result<i64, crate::JsonError>| r.map_err(|e| e.to_string());
        check!(text(JsonNumber::from("-42").int64()), Ok(-42), "int64")?;
        check!(
            text(JsonNumber::from("1.5").int64()),
            Err("strconv.ParseInt: parsing \"1.5\": invalid syntax".to_owned()),
            "int64 syntax",
        )?;
        check!(
            JsonNumber::from("0x1.8p1").float64().ok().map(f64::to_bits),
            Some(3.0_f64.to_bits()),
            "float64 hex",
        )?;
        Ok(())
    }

    #[test]
    fn adapter_round_trip() -> Result<(), String> {
        // adapters/adtarget: jsonutil accepts a string `aid`, json.Marshal rejects it.
        let mut n = JsonNumber::default();
        jsonutil::unmarshal(br#""some string""#, &mut n).map_err(|e| e.to_string())?;
        check!(n.string(), "some string", "decoded")?;
        check!(
            stdjson::marshal(&n).map_err(|e| e.to_string()),
            Err("json: invalid number literal \"some string\"".to_owned()),
            "json.Marshal",
        )?;
        check!(
            jsonutil::marshal(&n).map_err(|e| e.message),
            Ok(b"some string".to_vec()),
            "jsonutil.Marshal",
        )
    }
}
