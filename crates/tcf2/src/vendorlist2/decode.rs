//! Go `encoding/json` decoding (`decode.go`, `fold.go`) specialised to go-gdpr's
//! `vendorListContract`, the only type `vendorlist2.ParseEagerly` unmarshals.
//!
//! The decoder runs only after [`check_valid`] accepted the input, as Go's `Unmarshal` does, so
//! it walks a well-formed document. It reproduces the decoding rules that are observable through
//! `ParseEagerly`:
//!
//! - errors are saved and decoding goes on; the first saved error is returned;
//! - `null` empties maps and slices but leaves numbers and structs untouched;
//! - object keys match struct fields exactly or case-insensitively (Go's `foldName`, under which
//!   `ſ` matches `s` and the Kelvin sign matches `k`); unknown keys are skipped, and a repeated key
//!   decodes into the same field again;
//! - each map entry decodes into a fresh zero value that then replaces any earlier entry with the
//!   same key, and a second `vendors` object adds to the existing map;
//! - a JSON array decoded into an existing `[]uint8` reuses its backing array, so elements that are
//!   `null` or fail to decode keep whatever an earlier value left there;
//! - a JSON string decoded into a `[]uint8` is standard base64;
//! - strings are unescaped as `unquoteBytes` does, with each invalid UTF-8 byte and each unpaired
//!   surrogate escape becoming U+FFFD.

use std::{collections::HashMap, str};

use super::{VendorListError, scanner::check_valid};
use crate::base64::STD_ENCODING;

/// The Go type names in `UnmarshalTypeError` messages.
const CONTRACT_TYPE: &str = "vendorlist2.vendorListContract";
const VENDORS_TYPE: &str = "map[string]vendorlist2.vendorListVendorContract";
const VENDOR_TYPE: &str = "vendorlist2.vendorListVendorContract";
const BYTES_TYPE: &str = "[]uint8";

/// Go: `utf8.RuneError`.
const RUNE_ERROR: char = char::REPLACEMENT_CHARACTER;

/// A Go `[]uint8` field.
///
/// `backing` is the slice's backing array as far as it has been written since it was last
/// replaced (Go zeroes the rest), and `len` the slice length.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ByteSlice {
    backing: Vec<u8>,
    len: usize,
}

impl ByteSlice {
    /// The slice's elements.
    pub(super) fn values(&self) -> &[u8] {
        self.backing.get(..self.len).unwrap_or_default()
    }
}

/// Go: `vendorListVendorContract`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct VendorContract {
    pub(super) id: u16,
    pub(super) purposes: ByteSlice,
    pub(super) legitimate_interests: ByteSlice,
    pub(super) flexible_purposes: ByteSlice,
    pub(super) special_purposes: ByteSlice,
    pub(super) special_features: ByteSlice,
}

/// Go: the `map[string]vendorListVendorContract` field. Entries keep the position of their key's
/// first insertion.
#[derive(Debug, Clone, Default)]
pub(super) struct VendorMap {
    entries: Vec<VendorContract>,
    index: HashMap<Vec<u8>, usize>,
}

impl VendorMap {
    /// Go: `reflect.Value.SetMapIndex`.
    fn insert(&mut self, key: Vec<u8>, vendor: VendorContract) {
        if let Some(entry) = self
            .index
            .get(&key)
            .and_then(|&position| self.entries.get_mut(position))
        {
            *entry = vendor;
            return;
        }
        let _previous = self.index.insert(key, self.entries.len());
        self.entries.push(vendor);
    }

    /// The map's values, in the order their keys were first inserted.
    pub(super) fn values(&self) -> &[VendorContract] {
        &self.entries
    }
}

/// Go: `vendorListContract`.
#[derive(Debug, Clone, Default)]
pub(super) struct Contract {
    pub(super) gvl_specification_version: u16,
    pub(super) version: u16,
    pub(super) vendors: VendorMap,
}

/// Go: `json.Unmarshal(data, &contract)`.
pub(super) fn unmarshal(data: &[u8]) -> Result<Contract, VendorListError> {
    check_valid(data).map_err(VendorListError::Syntax)?;
    let mut decoder = Decoder {
        data,
        off: 0,
        saved_error: None,
    };
    let contract = decoder.contract();
    decoder.saved_error.map_or(Ok(contract), Err)
}

/// Go: `decodeState.errorContext` at the point an error is saved: the struct field being
/// decoded, if any.
#[derive(Debug, Clone, Copy)]
enum Context {
    /// The top-level value.
    None,
    /// A `vendorListContract` field, by JSON name.
    Contract(&'static str),
    /// A `vendorListVendorContract` field (inside `vendors`), by JSON name.
    Vendor(&'static str),
}

impl Context {
    /// Go: `addErrorContext`: `Struct.Field`, with the field path of JSON names.
    fn struct_field(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Contract(field) => Some(format!("vendorListContract.{field}")),
            Self::Vendor(field) => Some(format!("vendorListVendorContract.vendors.{field}")),
        }
    }
}

/// The fields of `vendorListContract`, by JSON name.
const CONTRACT_FIELDS: [&str; 3] = ["gvlSpecificationVersion", "vendorListVersion", "vendors"];

/// The fields of `vendorListVendorContract`, by JSON name.
const VENDOR_FIELDS: [&str; 6] = [
    "id",
    "purposes",
    "legIntPurposes",
    "flexiblePurposes",
    "specialPurposes",
    "specialFeatures",
];

/// Go: `decodeState`, reading a document `check_valid` accepted.
#[derive(Debug)]
struct Decoder<'a> {
    data: &'a [u8],
    off: usize,
    saved_error: Option<VendorListError>,
}

impl<'a> Decoder<'a> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.off).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.off += 1;
        }
    }

    /// Consumes `c` if it is the next byte.
    fn eat(&mut self, c: u8) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.off += 1;
        }
        found
    }

    /// Go: `rescanLiteral`: consumes the string, number, `true`, `false` or `null` at the
    /// cursor and returns its bytes.
    fn literal(&mut self) -> &'a [u8] {
        let start = self.off;
        if self.eat(b'"') {
            while let Some(c) = self.peek() {
                self.off += 1;
                match c {
                    b'"' => break,
                    b'\\' => self.off += 1,
                    _ => {}
                }
            }
        } else {
            while matches!(
                self.peek(),
                Some(b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'+' | b'-' | b'.')
            ) {
                self.off += 1;
            }
        }
        self.off = self.off.min(self.data.len());
        self.data.get(start..self.off).unwrap_or_default()
    }

    /// Go: `decodeState.skip`, iteratively, since values can nest 10000 deep.
    fn skip(&mut self) {
        if !matches!(self.peek(), Some(b'{' | b'[')) {
            let _literal = self.literal();
            return;
        }
        let mut depth = 0_usize;
        while let Some(c) = self.peek() {
            match c {
                b'"' => {
                    let _literal = self.literal();
                }
                b'{' | b'[' => {
                    depth += 1;
                    self.off += 1;
                }
                b'}' | b']' => {
                    depth = depth.saturating_sub(1);
                    self.off += 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => self.off += 1,
            }
        }
    }

    /// Go: `decodeState.saveError`.
    fn save_error(&mut self, err: VendorListError) {
        if self.saved_error.is_none() {
            self.saved_error = Some(err);
        }
    }

    fn save_type_error(&mut self, value: String, type_name: &'static str, context: Context) {
        self.save_error(VendorListError::UnmarshalType {
            value,
            struct_field: context.struct_field(),
            type_name,
        });
    }

    /// Saves the `UnmarshalTypeError` Go reports for the value at the cursor when it cannot be
    /// stored in `type_name` at all, and skips the value. Not for `null`, which every type
    /// accepts.
    fn reject(&mut self, type_name: &'static str, context: Context) {
        let value = match self.peek() {
            Some(b'{') => "object",
            Some(b'[') => "array",
            Some(b'"') => "string",
            Some(b't' | b'f') => "bool",
            _ => "number",
        };
        self.skip();
        self.save_type_error(value.to_owned(), type_name, context);
    }

    /// Go: `decodeState.object` over the members of the object at the cursor: calls `member`
    /// with each unquoted key, positioned at its value, which `member` must consume.
    fn object<F: FnMut(&mut Self, Vec<u8>)>(&mut self, mut member: F) {
        self.off += 1;
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                // The closing brace of an empty object.
                self.off += 1;
                return;
            }
            let key = unquote(self.literal());
            self.skip_space();
            let _colon = self.eat(b':');
            self.skip_space();
            member(self, key);
            self.skip_space();
            if !self.eat(b',') {
                self.off += 1;
                return;
            }
        }
    }

    /// The top-level value, into a `vendorListContract`.
    fn contract(&mut self) -> Contract {
        let mut contract = Contract::default();
        self.skip_space();
        match self.peek() {
            Some(b'{') => self.object(|d, key| match field_index(&key, &CONTRACT_FIELDS) {
                Some(0) => {
                    let context = Context::Contract("gvlSpecificationVersion");
                    if let Some(value) = d.uint("uint16", context) {
                        contract.gvl_specification_version = value;
                    }
                }
                Some(1) => {
                    if let Some(value) = d.uint("uint16", Context::Contract("vendorListVersion")) {
                        contract.version = value;
                    }
                }
                Some(_) => d.vendors(&mut contract.vendors),
                None => d.skip(),
            }),
            Some(b'n') => {
                let _null = self.literal();
            }
            _ => self.reject(CONTRACT_TYPE, Context::None),
        }
        contract
    }

    /// The `vendors` field.
    fn vendors(&mut self, vendors: &mut VendorMap) {
        match self.peek() {
            Some(b'{') => self.object(|d, key| {
                let mut vendor = VendorContract::default();
                d.vendor(&mut vendor);
                vendors.insert(key, vendor);
            }),
            Some(b'n') => {
                let _null = self.literal();
                *vendors = VendorMap::default();
            }
            _ => self.reject(VENDORS_TYPE, Context::Contract("vendors")),
        }
    }

    /// One `vendors` map value.
    fn vendor(&mut self, vendor: &mut VendorContract) {
        match self.peek() {
            Some(b'{') => self.object(|d, key| {
                let Some(index) = field_index(&key, &VENDOR_FIELDS) else {
                    d.skip();
                    return;
                };
                let context = Context::Vendor(VENDOR_FIELDS.get(index).copied().unwrap_or(""));
                let slice = match index {
                    0 => {
                        if let Some(id) = d.uint("uint16", context) {
                            vendor.id = id;
                        }
                        return;
                    }
                    1 => &mut vendor.purposes,
                    2 => &mut vendor.legitimate_interests,
                    3 => &mut vendor.flexible_purposes,
                    4 => &mut vendor.special_purposes,
                    _ => &mut vendor.special_features,
                };
                d.bytes(slice, context);
            }),
            Some(b'n') => {
                let _null = self.literal();
            }
            _ => self.reject(VENDOR_TYPE, Context::Contract("vendors")),
        }
    }

    /// A `[]uint8` field.
    fn bytes(&mut self, slice: &mut ByteSlice, context: Context) {
        match self.peek() {
            Some(b'[') => self.array(slice, context),
            Some(b'"') => {
                let text = unquote(self.literal());
                match STD_ENCODING.decode(&text) {
                    Ok(decoded) => {
                        slice.len = decoded.len();
                        slice.backing = decoded;
                    }
                    Err(err) => self.save_error(VendorListError::Base64(err)),
                }
            }
            Some(b'n') => {
                let _null = self.literal();
                *slice = ByteSlice::default();
            }
            _ => self.reject(BYTES_TYPE, context),
        }
    }

    /// Go: `decodeState.array` into a `[]uint8`.
    fn array(&mut self, slice: &mut ByteSlice, context: Context) {
        self.off += 1;
        let mut i = 0;
        loop {
            self.skip_space();
            if self.eat(b']') {
                break;
            }
            // Go grows the slice with zeroed memory but keeps the old backing array's contents.
            if i >= slice.backing.len() {
                slice.backing.push(0);
            }
            if i >= slice.len {
                slice.len = i + 1;
            }
            if let Some(value) = self.uint("uint8", context)
                && let Some(element) = slice.backing.get_mut(i)
            {
                *element = value;
            }
            i += 1;
            self.skip_space();
            if !self.eat(b',') {
                self.off += 1;
                break;
            }
        }
        if i < slice.len {
            slice.len = i;
        }
        if i == 0 {
            *slice = ByteSlice::default();
        }
    }

    /// An unsigned integer (Go `uint8` or `uint16`, named `type_name`): `None` when the value is
    /// `null` or cannot be stored.
    fn uint<T: TryFrom<u64>>(&mut self, type_name: &'static str, context: Context) -> Option<T> {
        match self.peek() {
            Some(b'n') => {
                let _null = self.literal();
                None
            }
            Some(b'-' | b'0'..=b'9') => {
                let literal = self.literal();
                let value = parse_uint(literal).and_then(|n| T::try_from(n).ok());
                if value.is_none() {
                    let literal = String::from_utf8_lossy(literal);
                    self.save_type_error(format!("number {literal}"), type_name, context);
                }
                value
            }
            _ => {
                self.reject(type_name, context);
                None
            }
        }
    }
}

/// Go: `strconv.ParseUint(s, 10, 64)`.
fn parse_uint(literal: &[u8]) -> Option<u64> {
    if literal.is_empty() {
        return None;
    }
    literal.iter().try_fold(0_u64, |n, &c| {
        if c.is_ascii_digit() {
            n.checked_mul(10)?.checked_add(u64::from(c - b'0'))
        } else {
            None
        }
    })
}

/// Go: `fields.byExactName[key]`, then `fields.byFoldedName[foldName(key)]`: the index of the
/// field `key` names. The field names are ASCII and distinct under folding, so a folded
/// comparison decides both lookups.
fn field_index(key: &[u8], names: &[&str]) -> Option<usize> {
    names.iter().position(|name| folds_to(key, name))
}

/// Whether Go's `foldName(key)` equals `foldName(name)` for an ASCII `name`.
fn folds_to(key: &[u8], name: &str) -> bool {
    let mut expected = name.bytes().map(|c| c.to_ascii_uppercase());
    let mut rest = key;
    while let Some(&c) = rest.first() {
        let folded = if c.is_ascii() {
            rest = rest.get(1..).unwrap_or_default();
            c.to_ascii_uppercase()
        } else {
            let (rune, size) = decode_rune(rest);
            rest = rest.get(size..).unwrap_or_default();
            // Go: `foldRune`, which maps a rune to the smallest one in its case-folding orbit.
            // Only these two non-ASCII runes fold to ASCII letters.
            match rune {
                '\u{17F}' => b'S',
                '\u{212A}' => b'K',
                _ => return false,
            }
        };
        if expected.next() != Some(folded) {
            return false;
        }
    }
    expected.next().is_none()
}

/// Go: `utf8.DecodeRune`: the first rune of `bytes` and its encoded length. Anything that does
/// not start with a complete, valid UTF-8 sequence decodes as `RuneError` with length 1.
fn decode_rune(bytes: &[u8]) -> (char, usize) {
    let size = match bytes.first() {
        None => return (RUNE_ERROR, 0),
        Some(0x00..=0x7F) => 1,
        Some(0xC2..=0xDF) => 2,
        Some(0xE0..=0xEF) => 3,
        Some(0xF0..=0xF4) => 4,
        _ => return (RUNE_ERROR, 1),
    };
    bytes
        .get(..size)
        .and_then(|prefix| str::from_utf8(prefix).ok())
        .and_then(|text| text.chars().next())
        .map_or((RUNE_ERROR, 1), |rune| (rune, size))
}

/// Go: `getu4`: the value of the `\uXXXX` escape at the start of `bytes`.
fn getu4(bytes: &[u8]) -> Option<u32> {
    match bytes {
        [b'\\', b'u', digits @ ..] => {
            let hex = str::from_utf8(digits.get(..4)?).ok()?;
            if hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Go: `utf16.DecodeRune`.
fn decode_surrogates(high: u32, low: u32) -> Option<char> {
    if (0xD800..0xDC00).contains(&high) && (0xDC00..0xE000).contains(&low) {
        char::from_u32((((high - 0xD800) << 10) | (low - 0xDC00)) + 0x1_0000)
    } else {
        None
    }
}

fn push_rune(out: &mut Vec<u8>, rune: char) {
    let mut buffer = [0; 4];
    out.extend_from_slice(rune.encode_utf8(&mut buffer).as_bytes());
}

/// Go: `unquoteBytes` for a string literal the scanner accepted.
fn unquote(literal: &[u8]) -> Vec<u8> {
    let mut rest = literal
        .get(1..literal.len().saturating_sub(1))
        .unwrap_or_default();
    let mut out = Vec::with_capacity(rest.len());
    while let Some(&c) = rest.first() {
        if c == b'\\' {
            if rest.get(1) == Some(&b'u') {
                let Some(code) = getu4(rest) else { break };
                rest = rest.get(6..).unwrap_or_default();
                let rune = if (0xD800..0xE000).contains(&code) {
                    // Go: `utf16.IsSurrogate`; the next escape is consumed only if it completes
                    // a pair.
                    let pair = getu4(rest).and_then(|low| decode_surrogates(code, low));
                    if pair.is_some() {
                        rest = rest.get(6..).unwrap_or_default();
                    }
                    pair.unwrap_or(RUNE_ERROR)
                } else {
                    char::from_u32(code).unwrap_or(RUNE_ERROR)
                };
                push_rune(&mut out, rune);
            } else {
                let Some(&escaped) = rest.get(1) else { break };
                out.push(match escaped {
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    // `"`, `\`, `/` (and `'`, which the scanner never lets through).
                    other => other,
                });
                rest = rest.get(2..).unwrap_or_default();
            }
        } else if c.is_ascii() {
            out.push(c);
            rest = rest.get(1..).unwrap_or_default();
        } else {
            let (rune, size) = decode_rune(rest);
            push_rune(&mut out, rune);
            rest = rest.get(size..).unwrap_or_default();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{decode_rune, folds_to, parse_uint, unquote};
    use crate::test_support::TestResult;

    #[test]
    fn go_decode_rune() -> TestResult {
        // A truncated three-byte sequence is one invalid byte at a time, unlike Rust's lossy
        // decoding which replaces the maximal invalid prefix once.
        check_eq!(decode_rune(b"\xE2\x84A"), ('\u{FFFD}', 1), "truncated");
        check_eq!(decode_rune(b"\xE2\x84\xAA"), ('\u{212A}', 3), "kelvin");
        check_eq!(decode_rune(b"\xED\xA0\x80"), ('\u{FFFD}', 1), "surrogate");
        check_eq!(decode_rune(b"\xC0\x80"), ('\u{FFFD}', 1), "overlong");
        check_eq!(decode_rune(b""), ('\u{FFFD}', 0), "empty");
        Ok(())
    }

    /// A JSON string literal whose body is `parts` joined, where `"u+XXXX"` stands for the
    /// escape of code unit `XXXX` (spelled this way so that no tool rewrites the escapes).
    fn quoted(parts: &[&str]) -> Vec<u8> {
        let mut out = String::from("\"");
        for part in parts {
            match part.strip_prefix("u+") {
                Some(hex) => {
                    out.push('\\');
                    out.push('u');
                    out.push_str(hex);
                }
                None => out.push_str(part),
            }
        }
        out.push('"');
        out.into_bytes()
    }

    #[test]
    fn go_unquote() -> TestResult {
        check_eq!(
            unquote(br#""a\"\\\/\b\f\n\r\t""#),
            b"a\"\\/\x08\x0C\n\r\t",
            "escapes"
        );
        check_eq!(
            unquote(&quoted(&["u+00e9", "u+D83D", "u+de00"])),
            "\u{E9}\u{1F600}".as_bytes(),
            "unicode"
        );
        check_eq!(
            unquote(&quoted(&["u+d800", "x"])),
            "\u{FFFD}x".as_bytes(),
            "lone high"
        );
        check_eq!(
            unquote(&quoted(&["u+d800", "u+0041"])),
            "\u{FFFD}A".as_bytes(),
            "bad pair"
        );
        check_eq!(
            unquote(&quoted(&["u+dc00"])),
            "\u{FFFD}".as_bytes(),
            "lone low"
        );
        check_eq!(unquote(&quoted(&["u+0000"])), b"\0", "NUL");
        check_eq!(
            unquote(b"\"\xE2\x84A\""),
            "\u{FFFD}\u{FFFD}A".as_bytes(),
            "invalid utf8"
        );
        Ok(())
    }

    #[test]
    fn go_fold_name() -> TestResult {
        check!(
            folds_to(b"VendorListVersion", "vendorListVersion"),
            "ASCII case"
        );
        check!(
            folds_to("\u{17F}pecialFeatures".as_bytes(), "specialFeatures"),
            "long s"
        );
        check!(folds_to("\u{212A}".as_bytes(), "k"), "kelvin");
        check!(!folds_to("\u{131}d".as_bytes(), "id"), "dotless i");
        check!(!folds_to(b"ids", "id"), "longer");
        check!(!folds_to(b"i", "id"), "shorter");
        Ok(())
    }

    #[test]
    fn go_parse_uint() -> TestResult {
        check_eq!(parse_uint(b"18446744073709551615"), Some(u64::MAX), "max");
        check_eq!(parse_uint(b"18446744073709551616"), None, "overflow");
        check_eq!(parse_uint(b"-0"), None, "sign");
        check_eq!(parse_uint(b"1e0"), None, "exponent");
        check_eq!(parse_uint(b""), None, "empty");
        Ok(())
    }
}
