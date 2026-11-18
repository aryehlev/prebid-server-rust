//! Go `gopkg.in/yaml.v3` v3.0.1 decoding: [`unmarshal`] (Go: `yaml.Unmarshal`) and
//! [`StreamDecoder`] (Go: `yaml.NewDecoder(...).Decode`).
//!
//! The libyaml port (reader, scanner, parser) and the composer and decoder of `decode.go` keep
//! Go's control flow, so the same inputs produce the same values and the same error texts:
//! syntax errors (`yaml: line 3: mapping values are not allowed in this context`), type errors
//! collected into [`TypeError`] (`yaml: unmarshal errors:\n  line 2: cannot unmarshal !!str
//! `abc` into int`), and the fatal decoding errors (`yaml: unknown anchor 'a' referenced`,
//! `yaml: document contains excessive aliasing`, ...).
//!
//! # Decoding targets
//!
//! A target implements [`YamlDecode`], which stands for a Go type whose name
//! ([`YamlDecode::yaml_type_name`]) appears in the errors. The Rust ↔ Go mapping is
//! `pbs-gojson`'s: `Option<T>` is `*T` (but a nil-able slice or map for `Option<Vec<T>>` and
//! `Option<BTreeMap<K, V>>`), `Box<T>` is `T`, `Vec<T>` is `[]T`, `BTreeMap`/`HashMap` are
//! `map[K]V`, [`Value`](crate::Value) is `interface{}`, `isize`/`usize` are `int`/`uint`,
//! [`GoString`](pbs_gojson::GoString) and `String` are `string`,
//! [`Duration`](pbs_gocompat::time::Duration) and [`Time`](pbs_gocompat::time::Time) are
//! `time.Duration` and `time.Time`. Structs and named types derive
//! [`GoYaml`](crate::GoYaml).
//!
//! A Go `Unmarshal` call is synchronous and recursive, so very deeply nested documents (Go
//! accepts up to 10000 levels of flow nesting) need a correspondingly large stack.
//!
//! # Not ported
//!
//! - `yaml.Node` and the `Unmarshaler`/`obsoleteUnmarshaler` interfaces; of
//!   `encoding.TextUnmarshaler`, only `time.Time`'s.
//! - The encoder (`yaml.Marshal`).
//! - Decoding into a map type nested in an `interface{}` value takes Go's default map types; Go
//!   switches them to the target's own `map[string]interface{}`-shaped named type
//!   (`d.stringMapType`), which only a named map type can observe.
//! - `!!binary` data is kept as a Rust `String`, so invalid UTF-8 in it is replaced (decode into
//!   [`GoString`](pbs_gojson::GoString) to keep the bytes).
//! - Go runtime panics that yaml.v3 does not recover (a struct with an invalid `yaml` tag, an
//!   unhashable merge key) are returned as [`UnmarshalError::Panic`] rather than crashing.

mod compose;
mod decode;
mod parserc;
mod readerc;
mod resolve;
mod scannerc;
mod yamlh;
mod yamlprivateh;

use std::{error, fmt};

pub use decode::{
    Decoder, FieldDesc, NodeId, Scalar, StructInfo, YamlDecode, YamlInline, YamlInlineMap,
    YamlStruct,
};

use self::compose::Composer;

/// Go: `*yaml.TypeError`: the decoding finished, but some values could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeError {
    /// One message per value, such as `line 2: cannot unmarshal !!str `abc` into int` (bytes,
    /// since Go truncates long values in the middle of a UTF-8 sequence).
    pub errors: Vec<Vec<u8>>,
}

/// A `yaml.Unmarshal` error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnmarshalError {
    /// Go: `*yaml.TypeError`.
    Type(TypeError),
    /// A fatal error, recovered by yaml.v3's `handleErr`: Go's `err.Error()`.
    Fatal(Vec<u8>),
    /// A Go runtime panic that yaml.v3 does not recover (the Go program would crash).
    Panic(String),
}

impl UnmarshalError {
    /// Go's `err.Error()` bytes.
    #[must_use]
    pub fn text(&self) -> Vec<u8> {
        match self {
            Self::Type(e) => {
                let mut out = b"yaml: unmarshal errors:\n  ".to_vec();
                for (i, msg) in e.errors.iter().enumerate() {
                    if i > 0 {
                        out.extend_from_slice(b"\n  ");
                    }
                    out.extend_from_slice(msg);
                }
                out
            }
            Self::Fatal(text) => text.clone(),
            Self::Panic(text) => text.clone().into_bytes(),
        }
    }

    /// A fatal error of Go's `failf`, which prefixes `yaml: `.
    pub(crate) fn failf(msg: &str) -> Self {
        let mut text = b"yaml: ".to_vec();
        text.extend_from_slice(msg.as_bytes());
        Self::Fatal(text)
    }
}

impl fmt::Display for UnmarshalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.text()))
    }
}

impl error::Error for UnmarshalError {}

/// Go: `yaml.Unmarshal(input, out)`: decodes the first document of `input` into `out`.
///
/// As in Go, values that cannot be decoded are skipped and reported together in
/// [`UnmarshalError::Type`] after the rest has been decoded; other errors stop the decoding,
/// leaving `out` partly written.
///
/// # Errors
///
/// The [`UnmarshalError`] Go would return.
pub fn unmarshal<T: YamlDecode + ?Sized>(input: &[u8], out: &mut T) -> Result<(), UnmarshalError> {
    let input: &[u8] = if input.is_empty() { b"\n" } else { input };
    let mut composer = Composer::new(input);
    let root = composer.parse()?;
    decode_root(&composer.nodes, root, out, false)
}

/// Decodes a composed document into `out` (Go: the body of `unmarshal` after `p.parse()`).
fn decode_root<T: YamlDecode + ?Sized>(
    nodes: &[compose::Node],
    root: Option<NodeId>,
    out: &mut T,
    known_fields: bool,
) -> Result<(), UnmarshalError> {
    let mut d = Decoder::new(nodes, known_fields);
    if let Some(root) = root {
        let _good = d.unmarshal(root, out)?;
    }
    d.finish()
}

/// Go: `*yaml.Decoder` over an in-memory stream: decodes one document per call.
#[derive(Debug)]
pub struct StreamDecoder<'a> {
    composer: Composer<'a>,
    known_fields: bool,
}

impl<'a> StreamDecoder<'a> {
    /// Go: `yaml.NewDecoder(bytes.NewReader(input))`.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            composer: Composer::new(input),
            known_fields: false,
        }
    }

    /// Go: `Decoder.KnownFields(enable)`: report mapping keys that match no struct field.
    pub const fn known_fields(&mut self, enable: bool) {
        self.known_fields = enable;
    }

    /// Go: `Decoder.Decode(out)`: decodes the next document; `Ok(false)` at the end of the
    /// stream (Go's `io.EOF`).
    ///
    /// # Errors
    ///
    /// The [`UnmarshalError`] Go would return.
    pub fn decode<T: YamlDecode + ?Sized>(&mut self, out: &mut T) -> Result<bool, UnmarshalError> {
        let Some(root) = self.composer.parse()? else {
            return Ok(false);
        };
        // Anchors outlive their document in Go, so the nodes of earlier documents are kept.
        decode_root(&self.composer.nodes, Some(root), out, self.known_fields)?;
        Ok(true)
    }
}
