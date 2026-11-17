//! Go-compatible JSON encoding in the three flavours Prebid Server uses.

mod impls;
#[doc(hidden)]
pub mod string;

pub use impls::{GoMapKey, encode_map, encode_slice};
pub(crate) use string::{html_escape, jsoniter_html_string, jsoniter_plain_string, std_string};

use crate::{
    error::JsonError,
    gotype::GoType,
    strconv::{FloatFmt, append_float, f64_to_f32, non_finite_text},
};

/// Which Go encoder an [`Encoder`] reproduces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flavor {
    /// `jsonutil.Marshal`: json-iterator's `ConfigCompatibleWithStandardLibrary` with Prebid's
    /// `RawMessageExtension` registered (as `main.go` does in production).
    ///
    /// HTML-escaped string values (control characters as `\u00XX`, no `\b`/`\f`), struct field
    /// names written without HTML escaping, floats without exponent clean-up (`1e-07`),
    /// `json.RawMessage` compacted, and a nil, empty or invalid `RawMessage` writes nothing at
    /// all (Go then produces invalid JSON such as `{"ext":}`). Errors are wrapped with Go field
    /// and type names and do not stop the encoding.
    Jsoniter,
    /// `encoding/json`: `json.Marshal` (`escape_html: true`) or a `json.Encoder` after
    /// `SetEscapeHTML(false)` (`escape_html: false`).
    ///
    /// `\b`/`\f` short escapes, floats with exponent clean-up (`1e-7`), nil `RawMessage` as
    /// `null`, and the first error aborts the whole encoding.
    Std {
        /// Whether `<`, `>` and `&` (and U+2028/U+2029 inside raw messages) are escaped.
        escape_html: bool,
    },
}

/// An output buffer plus Go's per-encode error state.
///
/// [`GoEncode`] implementations write through the `write_*` methods, which apply the flavour's
/// escaping and number formatting.
#[derive(Debug)]
pub struct Encoder {
    buf: Vec<u8>,
    flavor: Flavor,
    error: Option<Vec<u8>>,
}

impl Encoder {
    /// An empty encoder for `flavor`.
    #[must_use]
    pub const fn new(flavor: Flavor) -> Self {
        Self {
            buf: Vec::new(),
            flavor,
            error: None,
        }
    }

    /// The flavour being reproduced.
    #[must_use]
    pub const fn flavor(&self) -> Flavor {
        self.flavor
    }

    /// Whether the jsoniter flavour is active.
    #[must_use]
    pub const fn is_jsoniter(&self) -> bool {
        matches!(self.flavor, Flavor::Jsoniter)
    }

    /// Whether HTML escaping is on (always true for the jsoniter flavour).
    #[must_use]
    pub const fn escape_html(&self) -> bool {
        match self.flavor {
            Flavor::Jsoniter => true,
            Flavor::Std { escape_html } => escape_html,
        }
    }

    /// The bytes written so far.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Number of bytes written so far.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing has been written.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Appends bytes verbatim.
    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Appends one byte verbatim.
    pub fn write_byte(&mut self, b: u8) {
        self.buf.push(b);
    }

    /// Writes `null`.
    pub fn write_null(&mut self) {
        self.buf.extend_from_slice(b"null");
    }

    /// Writes `true` or `false`.
    pub fn write_bool(&mut self, v: bool) {
        let text: &[u8] = if v { b"true" } else { b"false" };
        self.buf.extend_from_slice(text);
    }

    /// Writes a signed integer.
    pub fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(v.to_string().as_bytes());
    }

    /// Writes an unsigned integer.
    pub fn write_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(v.to_string().as_bytes());
    }

    /// Writes a Go `float64`.
    pub fn write_f64(&mut self, v: f64) {
        self.write_float(v, false);
    }

    /// Writes a Go `float32`.
    pub fn write_f32(&mut self, v: f32) {
        self.write_float(f64::from(v), true);
    }

    fn write_float(&mut self, v: f64, bits32: bool) {
        if !v.is_finite() {
            // Go: jsoniter stream_float.go WriteFloat64 / encoding/json floatEncoder.encode
            let text = non_finite_text(v);
            match self.flavor {
                Flavor::Jsoniter => {
                    self.set_error(format!("unsupported value: {text}").into_bytes());
                }
                Flavor::Std { .. } => {
                    self.set_error(format!("json: unsupported value: {text}").into_bytes());
                }
            }
            return;
        }
        let abs = v.abs();
        let exponent = if bits32 {
            let a = f64_to_f32(abs);
            a != 0.0 && (a < 1e-6 || a >= 1e21)
        } else {
            abs != 0.0 && (abs < 1e-6 || abs >= 1e21)
        };
        let fmt = if exponent { FloatFmt::E } else { FloatFmt::F };
        let start = self.buf.len();
        append_float(&mut self.buf, v, fmt, bits32);
        if exponent && matches!(self.flavor, Flavor::Std { .. }) {
            // Go: encoding/json floatEncoder: clean up e-09 to e-9.
            let n = self.buf.len();
            if n >= start + 4
                && self.buf.get(n - 4) == Some(&b'e')
                && self.buf.get(n - 3) == Some(&b'-')
                && self.buf.get(n - 2) == Some(&b'0')
            {
                let last = self.buf.get(n - 1).copied().unwrap_or(b'0');
                if let Some(slot) = self.buf.get_mut(n - 2) {
                    *slot = last;
                }
                self.buf.truncate(n - 1);
            }
        }
    }

    /// Writes a Go string value (or map key) with the flavour's escaping.
    pub fn write_str(&mut self, s: &[u8]) {
        match self.flavor {
            Flavor::Jsoniter => jsoniter_html_string(&mut self.buf, s),
            Flavor::Std { escape_html } => std_string(&mut self.buf, s, escape_html),
        }
    }

    /// Writes a struct field name followed by `:` the way the flavour writes them.
    pub fn write_field_name(&mut self, name: &str) {
        match self.flavor {
            // Go: jsoniter stream.go WriteObjectField → WriteString
            Flavor::Jsoniter => jsoniter_plain_string(&mut self.buf, name.as_bytes()),
            // Go: encoding/json typeFields nameEscHTML / nameNonEsc
            Flavor::Std { escape_html } => {
                self.buf.push(b'"');
                if escape_html {
                    html_escape(&mut self.buf, name.as_bytes());
                } else {
                    self.buf.extend_from_slice(name.as_bytes());
                }
                self.buf.push(b'"');
            }
        }
        self.buf.push(b':');
    }

    /// Whether an error has been recorded.
    #[must_use]
    pub const fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Records an error (`err.Error()` text). jsoniter overwrites the current error, the way its
    /// leaf encoders assign `stream.Error`; `encoding/json` keeps the first error, since it stops
    /// at the first one.
    pub fn set_error(&mut self, text: Vec<u8>) {
        match self.flavor {
            Flavor::Jsoniter => self.error = Some(text),
            Flavor::Std { .. } => {
                if self.error.is_none() {
                    self.error = Some(text);
                }
            }
        }
    }

    /// jsoniter only: prefixes the current error with `prefix` (Go's
    /// `fmt.Errorf("%s: %s", name, err)` and `fmt.Errorf("%v.%s", typ, err)` wrapping).
    pub fn wrap_error(&mut self, prefix: &str, separator: &str) {
        if !self.is_jsoniter() {
            return;
        }
        if let Some(err) = self.error.as_mut() {
            let mut wrapped = Vec::with_capacity(prefix.len() + separator.len() + err.len());
            wrapped.extend_from_slice(prefix.as_bytes());
            wrapped.extend_from_slice(separator.as_bytes());
            wrapped.extend_from_slice(err);
            *err = wrapped;
        }
    }

    /// jsoniter only: wraps the current error with a Go type name (`"%v: %s"`), computed lazily.
    pub fn wrap_error_with_type<T: GoType + ?Sized>(&mut self, separator: &str) {
        if self.is_jsoniter() && self.error.is_some() {
            let mut name = String::new();
            T::go_type_name(&mut name);
            self.wrap_error(&name, separator);
        }
    }

    /// The recorded error text, if any.
    #[must_use]
    pub fn error_text(&self) -> Option<&[u8]> {
        self.error.as_deref()
    }

    pub(crate) const fn take_error(&mut self) -> Option<Vec<u8>> {
        self.error.take()
    }

    pub(crate) fn adopt_error_if_unset(&mut self, err: Option<Vec<u8>>) {
        if self.error.is_none() {
            self.error = err;
        }
    }

    /// Finishes the encoding: the output, or the recorded error.
    ///
    /// # Errors
    ///
    /// Returns the recorded error, if any, as Go's `Marshal` would.
    pub fn finish(self) -> Result<Vec<u8>, JsonError> {
        match self.error {
            Some(err) => Err(JsonError::Message(err)),
            None => Ok(self.buf),
        }
    }
}

/// A Rust value that encodes exactly like the Go value it stands for.
///
/// Implemented for the type mapping documented in [`crate::gotype`], for
/// [`RawJson`](crate::RawJson), `serde_json::Value`, and by [`#[derive(GoJson)]`](crate::GoJson).
pub trait GoEncode: GoType {
    /// Encodes the value as Go's encoder for the active flavour would.
    fn go_encode(&self, enc: &mut Encoder);

    /// Go's `omitempty` test: `false`, `0`, `""`, nil pointers and interfaces, and empty
    /// slices and maps are empty; structs never are.
    fn go_is_empty(&self) -> bool;

    /// `encoding/json` encoding for a field tagged `,string`. Booleans, numbers and strings are
    /// written inside a JSON string; other types encode normally.
    #[doc(hidden)]
    fn go_encode_quoted(&self, enc: &mut Encoder) {
        self.go_encode(enc);
    }

    /// The `omitempty` test for `Option<Self>`: nil pointers are empty (slices and maps also
    /// treat an empty value as empty).
    #[doc(hidden)]
    fn option_is_empty(value: &Option<Self>) -> bool
    where
        Self: Sized,
    {
        value.is_none()
    }

    /// Encodes `Option<Self>` (a pointer): `null` for `None`. `RawJson` overrides it, since
    /// jsoniter calls `MarshalJSON` on `*json.RawMessage` instead of compacting.
    #[doc(hidden)]
    fn option_go_encode(value: &Option<Self>, enc: &mut Encoder)
    where
        Self: Sized,
    {
        match value {
            None => enc.write_null(),
            Some(v) => v.go_encode(enc),
        }
    }

    /// Encodes `[]Self`. Go encodes `[]byte` (and only it) as base64.
    #[doc(hidden)]
    fn go_encode_slice(items: &[Self], enc: &mut Encoder)
    where
        Self: Sized,
    {
        encode_slice(items, enc);
    }
}

/// Encodes `value` with `flavor`, the way Go's top-level call would.
///
/// # Errors
///
/// Returns Go's encode error (`unsupported value: NaN`, a `MarshalJSON` failure, ...).
pub fn marshal_with<T: GoEncode + ?Sized>(value: &T, flavor: Flavor) -> Result<Vec<u8>, JsonError> {
    let mut enc = Encoder::new(flavor);
    value.go_encode(&mut enc);
    enc.finish()
}
