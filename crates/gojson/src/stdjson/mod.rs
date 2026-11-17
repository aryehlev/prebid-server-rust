//! `encoding/json` (Go 1.26) decoding and encoding: `json.Unmarshal`, `json.Marshal`, and a
//! `json.Encoder` with `SetEscapeHTML(false)`.
//!
//! Decoding checks the whole input first (`checkValid`), so syntax errors are reported before
//! anything is stored. Type mismatches are recorded as `UnmarshalTypeError`s with Go's struct
//! and field context, the first one is returned after decoding completes, and errors from
//! `UnmarshalJSON` (and misuse of `,string`) abort decoding.

mod impls;
#[doc(hidden)]
pub mod scanner;
#[doc(hidden)]
pub mod value;

pub use impls::{Class, Store, literal_store, std_decode_map, std_decode_scalar, std_decode_slice};
pub(crate) use scanner::append_compact;
pub use scanner::{compact, valid};

use crate::{
    encode::{Flavor, GoEncode, marshal_with},
    error::JsonError,
    gotype::GoType,
    strconv::quote,
    unicode::decode_rune,
};

/// A byte of a number or a `true`/`false`/`null` literal (the scanner has validated the input).
const fn is_literal_byte(c: u8) -> bool {
    matches!(c, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E' | b'a'..=b'z')
}

/// The kind of the next JSON value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    /// `[`
    Array,
    /// `{`
    Object,
    /// A string, number, `true`, `false` or `null`.
    Literal,
}

/// Go's `errorContext`: the innermost struct being decoded and the field path to the value.
#[derive(Clone, Debug, Default)]
struct ErrorContext {
    /// `Some` once a struct field has been entered (the name may be empty for anonymous
    /// structs).
    struct_name: Option<&'static str>,
    field_stack: Vec<String>,
}

/// The state of one `encoding/json` decode (`decodeState`) over input already checked for
/// syntax errors.
#[derive(Debug)]
pub struct StdDecoder<'a> {
    data: &'a [u8],
    off: usize,
    saved: Option<JsonError>,
    context: Option<ErrorContext>,
}

/// What `valueQuoted` found for a `,string` field.
pub(crate) enum Quoted {
    Null,
    Str(Vec<u8>),
    Unquoted,
}

impl<'a> StdDecoder<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            off: 0,
            saved: None,
            context: None,
        }
    }

    fn byte(&self) -> u8 {
        self.data.get(self.off).copied().unwrap_or(0)
    }

    fn skip_space(&mut self) {
        while matches!(self.byte(), b' ' | b'\t' | b'\r' | b'\n') {
            self.off += 1;
        }
    }

    /// The kind of the next value.
    pub fn peek(&mut self) -> Token {
        self.skip_space();
        match self.byte() {
            b'[' => Token::Array,
            b'{' => Token::Object,
            _ => Token::Literal,
        }
    }

    /// Consumes the next literal and returns its bytes (strings keep their quotes).
    pub fn read_literal(&mut self) -> &'a [u8] {
        self.skip_space();
        let start = self.off;
        if self.byte() == b'"' {
            self.off += 1;
            while self.off < self.data.len() {
                match self.byte() {
                    b'\\' => self.off += 2,
                    b'"' => {
                        self.off += 1;
                        break;
                    }
                    _ => self.off += 1,
                }
            }
        } else {
            while is_literal_byte(self.byte()) {
                self.off += 1;
            }
        }
        self.data.get(start..self.off).unwrap_or_default()
    }

    /// Skips the next value (iteratively).
    pub fn skip(&mut self) {
        let mut depth = 0_usize;
        loop {
            self.skip_space();
            match self.byte() {
                b'[' | b'{' => {
                    depth += 1;
                    self.off += 1;
                }
                b']' | b'}' => {
                    depth = depth.saturating_sub(1);
                    self.off += 1;
                }
                b',' | b':' => self.off += 1,
                _ => {
                    let _ = self.read_literal();
                }
            }
            if depth == 0 {
                return;
            }
        }
    }

    /// The raw bytes of the next value, consumed (what `UnmarshalJSON` receives).
    pub fn raw_value(&mut self) -> &'a [u8] {
        self.skip_space();
        let start = self.off;
        self.skip();
        self.data.get(start..self.off).unwrap_or_default()
    }

    /// Consumes `[`; false (with `]` consumed) when the array is empty.
    pub fn array_begin(&mut self) -> bool {
        self.skip_space();
        self.off += 1;
        self.skip_space();
        if self.byte() == b']' {
            self.off += 1;
            return false;
        }
        true
    }

    /// After an element: true (with `,` consumed) when another element follows.
    pub fn array_next(&mut self) -> bool {
        self.skip_space();
        let c = self.byte();
        self.off += 1;
        c == b','
    }

    /// Consumes `{` and returns the first key literal (with its `:` consumed), if any.
    pub fn object_begin(&mut self) -> Option<&'a [u8]> {
        self.skip_space();
        self.off += 1;
        self.skip_space();
        if self.byte() == b'}' {
            self.off += 1;
            return None;
        }
        Some(self.object_key())
    }

    /// After a member value: the next key literal, or `None` (with `}` consumed).
    pub fn object_next(&mut self) -> Option<&'a [u8]> {
        self.skip_space();
        let c = self.byte();
        self.off += 1;
        if c == b',' {
            return Some(self.object_key());
        }
        None
    }

    fn object_key(&mut self) -> &'a [u8] {
        let key = self.read_literal();
        self.skip_space();
        self.off += 1;
        key
    }

    fn apply_context(&self, err: JsonError) -> JsonError {
        // Go: encoding/json/decode.go addErrorContext
        let Some(ctx) = &self.context else {
            return err;
        };
        if ctx.struct_name.is_none() && ctx.field_stack.is_empty() {
            return err;
        }
        match err {
            JsonError::UnmarshalType {
                value,
                go_type,
                offset,
                field,
                ..
            } => {
                let mut stack = ctx.field_stack.clone();
                if !field.is_empty() {
                    stack.push(field);
                }
                JsonError::UnmarshalType {
                    value,
                    go_type,
                    offset,
                    struct_name: struct_short_name(ctx.struct_name.unwrap_or_default()).to_owned(),
                    field: stack.join("."),
                }
            }
            other @ (JsonError::Syntax { .. } | JsonError::Message(_)) => other,
        }
    }

    /// Go's `saveError`: keeps the first error, with the current struct/field context.
    pub fn save_error(&mut self, err: JsonError) {
        if self.saved.is_none() {
            self.saved = Some(self.apply_context(err));
        }
    }

    /// Saves an `UnmarshalTypeError` for a JSON `value` kind that cannot go into a Go `N`.
    pub fn type_error<N: GoType + ?Sized>(&mut self, value: &str) {
        if self.saved.is_some() {
            return;
        }
        let mut go_type = String::new();
        N::go_type_name(&mut go_type);
        self.save_error(JsonError::UnmarshalType {
            value: value.to_owned(),
            go_type,
            offset: i64::try_from(self.off).unwrap_or(i64::MAX),
            struct_name: String::new(),
            field: String::new(),
        });
    }

    /// The `,string` misuse error for `item` going into a Go `N`.
    #[must_use]
    pub fn invalid_string_use<N: GoType + ?Sized>(item: &[u8]) -> JsonError {
        let mut text = b"json: invalid use of ,string struct tag, trying to unmarshal ".to_vec();
        text.extend_from_slice(&quote(item));
        text.extend_from_slice(b" into ");
        let mut name = String::new();
        N::go_type_name(&mut name);
        text.extend_from_slice(name.as_bytes());
        JsonError::Message(text)
    }

    /// Enters struct field `json_name` (after the intermediate embedded fields' Go names), as
    /// Go's `object` does before decoding a member. Returns the context to restore.
    pub(crate) fn enter_field(
        &mut self,
        struct_name: &'static str,
        embedded: &[&'static str],
        json_name: &str,
    ) -> (Option<&'static str>, usize) {
        let ctx = self.context.get_or_insert_with(ErrorContext::default);
        let restore = (ctx.struct_name, ctx.field_stack.len());
        ctx.field_stack
            .extend(embedded.iter().map(|name| (*name).to_owned()));
        ctx.field_stack.push(json_name.to_owned());
        ctx.struct_name = Some(struct_name);
        restore
    }

    /// The current context, to restore after a member (Go's `origErrorContext`).
    pub(crate) fn context_mark(&self) -> (Option<&'static str>, usize) {
        self.context
            .as_ref()
            .map_or((None, 0), |ctx| (ctx.struct_name, ctx.field_stack.len()))
    }

    /// Restores the context saved by [`StdDecoder::context_mark`].
    pub(crate) fn restore_context(&mut self, mark: (Option<&'static str>, usize)) {
        if let Some(ctx) = self.context.as_mut() {
            ctx.field_stack.truncate(mark.1);
            ctx.struct_name = mark.0;
        }
    }

    /// Go's `valueQuoted` for a `,string` member.
    pub(crate) fn value_quoted(&mut self) -> Quoted {
        match self.peek() {
            Token::Array | Token::Object => {
                self.skip();
                Quoted::Unquoted
            }
            Token::Literal => {
                let item = self.read_literal();
                match item.first() {
                    Some(b'n') => Quoted::Null,
                    Some(b'"') => Quoted::Str(unquote(item).unwrap_or_default()),
                    Some(b't' | b'f') | None => Quoted::Unquoted,
                    Some(_) => {
                        // literalInterface converts the number, saving its error.
                        let _number = value::convert_number(self, item);
                        Quoted::Unquoted
                    }
                }
            }
        }
    }
}

/// `reflect.Type.Name()` from a `pkg.Name` type string.
fn struct_short_name(go_type: &str) -> &str {
    go_type.rsplit_once('.').map_or(go_type, |(_, name)| name)
}

/// `getu4`: the value of a `\uXXXX` escape at the start of `s`.
fn getu4(s: &[u8]) -> Option<u32> {
    if s.len() < 6 || s.first() != Some(&b'\\') || s.get(1) != Some(&b'u') {
        return None;
    }
    let mut r: u32 = 0;
    for &c in s.get(2..6).unwrap_or_default() {
        let digit = char::from(c).to_digit(16)?;
        r = r * 16 + digit;
    }
    Some(r)
}

fn push_rune(out: &mut Vec<u8>, r: u32) {
    let c = char::from_u32(r).unwrap_or('\u{FFFD}');
    let mut buf = [0_u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Go's `unquoteBytes`: the unescaped contents of a JSON string literal, with invalid UTF-8 and
/// lone surrogates replaced by U+FFFD; `None` when the literal is malformed.
#[must_use]
pub fn unquote(item: &[u8]) -> Option<Vec<u8>> {
    // Go: encoding/json/decode.go unquoteBytes
    if item.len() < 2 || item.first() != Some(&b'"') || item.last() != Some(&b'"') {
        return None;
    }
    let s = item.get(1..item.len() - 1).unwrap_or_default();
    let mut out = Vec::with_capacity(s.len());
    let mut r = 0;
    while let Some(&c) = s.get(r) {
        match c {
            b'\\' => {
                let &e = s.get(r + 1)?;
                let simple = match e {
                    b'"' | b'\\' | b'/' | b'\'' => Some(e),
                    b'b' => Some(0x08),
                    b'f' => Some(0x0C),
                    b'n' => Some(b'\n'),
                    b'r' => Some(b'\r'),
                    b't' => Some(b'\t'),
                    b'u' => None,
                    _ => return None,
                };
                if let Some(b) = simple {
                    out.push(b);
                    r += 2;
                    continue;
                }
                let rr = getu4(s.get(r..).unwrap_or_default())?;
                r += 6;
                if (0xD800..0xE000).contains(&rr) {
                    let rr1 = getu4(s.get(r..).unwrap_or_default());
                    if let Some(rr1) = rr1
                        && (0xD800..0xDC00).contains(&rr)
                        && (0xDC00..0xE000).contains(&rr1)
                    {
                        r += 6;
                        push_rune(&mut out, ((rr - 0xD800) << 10 | (rr1 - 0xDC00)) + 0x10000);
                        continue;
                    }
                    push_rune(&mut out, 0xFFFD);
                    continue;
                }
                push_rune(&mut out, rr);
            }
            b'"' => return None,
            _ if c < b' ' => return None,
            _ if c < 0x80 => {
                out.push(c);
                r += 1;
            }
            _ => {
                let (rune, size) = decode_rune(s.get(r..).unwrap_or_default());
                r += size.max(1);
                let mut buf = [0_u8; 4];
                out.extend_from_slice(rune.unwrap_or('\u{FFFD}').encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    Some(out)
}

/// A Rust value that `encoding/json` decodes exactly like the Go value it stands for.
///
/// The `_as` methods take the Go type `N` that error messages name (a newtype names itself
/// rather than its underlying type).
pub trait StdDecode: GoType {
    /// Go's `value(v)`: decodes the next value, naming the Go type `N` in errors.
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding (an `UnmarshalJSON` failure or a `,string`
    /// misuse); type mismatches are saved in `d` instead.
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError>;

    /// Go's `literalStore(item, v, fromQuoted)` for this type.
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding.
    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError>;

    /// Go's `value(v)`: decodes the next value.
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding.
    fn std_decode(&mut self, d: &mut StdDecoder<'_>) -> Result<(), JsonError> {
        self.std_decode_as::<Self>(d)
    }

    /// `literalStore` naming the type itself.
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding.
    fn std_literal(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        self.std_literal_as::<Self>(item, from_quoted, d)
    }

    /// Decodes into `Option<Self>` (Go's pointer indirection: `null` sets nil, anything else
    /// allocates). Slices and maps override it, since `Option` models their nil state.
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding.
    #[doc(hidden)]
    fn std_decode_option(slot: &mut Option<Self>, d: &mut StdDecoder<'_>) -> Result<(), JsonError>
    where
        Self: Sized + Default,
    {
        match d.peek() {
            Token::Array | Token::Object => slot.get_or_insert_with(Self::default).std_decode(d),
            Token::Literal => {
                let item = d.read_literal();
                Self::std_literal_option(slot, item, false, d)
            }
        }
    }

    /// `literalStore` into `Option<Self>` (a pointer).
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding.
    #[doc(hidden)]
    fn std_literal_option(
        slot: &mut Option<Self>,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError>
    where
        Self: Sized + Default,
    {
        if item.is_empty() {
            d.save_error(StdDecoder::invalid_string_use::<Option<Self>>(item));
            return Ok(());
        }
        if item.first() == Some(&b'n') {
            if from_quoted && item != b"null" {
                d.save_error(StdDecoder::invalid_string_use::<Option<Self>>(item));
                return Ok(());
            }
            *slot = None;
            return Ok(());
        }
        slot.get_or_insert_with(Self::default)
            .std_literal_as::<Self>(item, from_quoted, d)
    }

    /// Decodes a nil-able `[]Self` (`[]byte` overrides it with base64).
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding.
    #[doc(hidden)]
    fn std_decode_slice<N: GoType + ?Sized>(
        slot: &mut Option<Vec<Self>>,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError>
    where
        Self: Sized + Default,
    {
        std_decode_slice::<N, Self>(slot, d)
    }

    /// `literalStore` into a nil-able `[]Self` (`null` → nil; `[]byte` decodes base64).
    ///
    /// # Errors
    ///
    /// Returns an error that aborts decoding.
    #[doc(hidden)]
    fn std_slice_literal<N: GoType + ?Sized>(
        slot: &mut Option<Vec<Self>>,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError>
    where
        Self: Sized + Default,
    {
        if matches!(
            literal_store::<N>(item, from_quoted, d, Class::Nilable)?,
            Store::Null
        ) {
            *slot = None;
        }
        Ok(())
    }
}

/// `json.Unmarshal(data, v)`.
///
/// # Errors
///
/// Returns the `SyntaxError` for malformed input, the error that aborted decoding, or the
/// first saved `UnmarshalTypeError`.
pub fn unmarshal<T: StdDecode + ?Sized>(data: &[u8], v: &mut T) -> Result<(), JsonError> {
    // Go: encoding/json/decode.go Unmarshal / decodeState.unmarshal
    scanner::check_valid(data)?;
    let mut d = StdDecoder::new(data);
    if let Err(err) = v.std_decode(&mut d) {
        return Err(d.apply_context(err));
    }
    d.saved.map_or(Ok(()), Err)
}

/// `json.Marshal(v)`.
///
/// # Errors
///
/// Returns Go's encode error (`json: unsupported value: NaN`, a `MarshalerError`, ...).
pub fn marshal<T: GoEncode + ?Sized>(v: &T) -> Result<Vec<u8>, JsonError> {
    marshal_with(v, Flavor::Std { escape_html: true })
}

/// `json.NewEncoder(w)` with `SetEscapeHTML(false)`, then `Encode(v)`: the encoding without
/// HTML escaping, followed by a newline.
///
/// # Errors
///
/// Returns Go's encode error; nothing is written in that case.
pub fn encode_no_escape_html<T: GoEncode + ?Sized>(v: &T) -> Result<Vec<u8>, JsonError> {
    let mut out = marshal_with(v, Flavor::Std { escape_html: false })?;
    out.push(b'\n');
    Ok(out)
}
