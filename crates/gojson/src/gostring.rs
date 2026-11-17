//! [`GoString`]: a Go `string`, i.e. an arbitrary byte string that is usually UTF-8.

use core::{fmt, ops::Deref, str::from_utf8};

use crate::{
    gotype::{GoKind, GoType},
    unicode::go_lossy,
};

/// A Go `string`: bytes that are usually, but not necessarily, valid UTF-8.
///
/// Use `String` for ordinary fields. Use `GoString` where Go would carry invalid UTF-8 through
/// unchanged and the difference is observable: jsoniter keeps invalid bytes from the input, and
/// every Go encoder later writes each of them as the escape `\ufffd`, whereas a Rust `String`
/// can only hold the already-replaced character U+FFFD, which the encoders write as raw bytes.
/// Error messages built from jsoniter errors (which quote raw input) are the main example.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GoString(Vec<u8>);

impl GoString {
    /// An empty string.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Wraps Go string bytes.
    #[must_use]
    pub const fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The string's bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the string, returning its bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// The string as UTF-8 with each invalid byte replaced by U+FFFD (what Go's `for range`
    /// iteration sees).
    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        go_lossy(&self.0)
    }

    /// The string as `&str` if it is valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        from_utf8(&self.0).ok()
    }

    /// Whether the string is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Length in bytes (Go's `len`).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }
}

impl Deref for GoString {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&str> for GoString {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl From<String> for GoString {
    fn from(s: String) -> Self {
        Self(s.into_bytes())
    }
}

impl From<Vec<u8>> for GoString {
    fn from(b: Vec<u8>) -> Self {
        Self(b)
    }
}

impl From<&[u8]> for GoString {
    fn from(b: &[u8]) -> Self {
        Self(b.to_vec())
    }
}

impl PartialEq<str> for GoString {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl PartialEq<&str> for GoString {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl fmt::Display for GoString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&go_lossy(&self.0))
    }
}

impl fmt::Debug for GoString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&go_lossy(&self.0), f)
    }
}

impl GoType for GoString {
    const KIND: GoKind = GoKind::String;

    fn go_type_name(out: &mut String) {
        out.push_str("string");
    }
}
