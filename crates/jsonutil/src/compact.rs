//! Prebid's `jsonutil.RawMessageExtension`, which `main.go` registers at startup: every
//! `json.RawMessage` is written through `json.Compact`, its whitespace between tokens removed and
//! nothing else changed (no HTML escaping, number spellings and escapes kept). `serde_json` writes
//! a `RawValue` verbatim, so [`raw_compact`] restores Go's bytes for a field whatever writes it,
//! and `marshal` compacts every raw value it writes (`json.raw-not-compacted`).

use std::{borrow::Cow, collections::BTreeMap};

use serde::{Serialize, Serializer, ser::Error as _};
use serde_json::value::RawValue;

/// A `serialize_with` helper that writes each `json.RawMessage` in a field compacted.
///
/// The field holds a `RawValue`, or an `Option`, `Vec`, `BTreeMap` or `Box` of them, and each raw
/// value is written as Go's `RawMessageExtension` writes it, whichever writer serializes the type
/// (`serde_json`, axum's `Json`, reqwest's `.json()`); `marshal` and `marshal_raw` compact raw
/// values without it. `None` is written as `null`; Go writes a nil `json.RawMessage` that
/// `omitempty` does not drop as nothing at all, which is not JSON (`json.nil-raw-written`).
///
/// # Errors
///
/// Whatever `serializer` reports.
pub fn raw_compact<T: CompactRaw + ?Sized, S: Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.serialize_compact(serializer)
}

/// A type [`raw_compact`] can write: raw JSON, or a container of it.
pub trait CompactRaw {
    /// Writes `self` with every raw value in it compacted.
    ///
    /// # Errors
    ///
    /// Whatever `serializer` reports.
    fn serialize_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;
}

impl CompactRaw for RawValue {
    fn serialize_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match compact(self.get()) {
            Cow::Borrowed(_) => self.serialize(serializer),
            Cow::Owned(text) => Self::from_string(text)
                .map_err(S::Error::custom)?
                .serialize(serializer),
        }
    }
}

impl<T: CompactRaw + ?Sized> CompactRaw for Box<T> {
    fn serialize_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (**self).serialize_compact(serializer)
    }
}

impl<T: CompactRaw> CompactRaw for Option<T> {
    fn serialize_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(value) => serializer.serialize_some(&Compact(value)),
            None => serializer.serialize_none(),
        }
    }
}

impl<T: CompactRaw> CompactRaw for Vec<T> {
    fn serialize_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(Compact))
    }
}

impl<K: Serialize, V: CompactRaw> CompactRaw for BTreeMap<K, V> {
    fn serialize_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter().map(|(key, value)| (key, Compact(value))))
    }
}

/// A [`CompactRaw`] value as a `Serialize` one.
struct Compact<'a, T: ?Sized>(&'a T);

impl<T: CompactRaw + ?Sized> Serialize for Compact<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize_compact(serializer)
    }
}

/// Go: `json.Compact` on valid JSON text: the spaces, tabs, carriage returns and line feeds
/// between tokens removed, everything inside strings kept. Text without such whitespace comes
/// back borrowed.
#[must_use]
pub fn compact(text: &str) -> Cow<'_, str> {
    let mut scan = Scan::default();
    let mut out = String::new();
    let mut copied = 0;
    for (at, byte) in text.bytes().enumerate() {
        if !scan.keeps(byte) {
            out.push_str(text.get(copied..at).unwrap_or_default());
            copied = at + 1;
        }
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(text.get(copied..).unwrap_or_default());
    Cow::Owned(out)
}

/// Where a scan of valid JSON text is: between tokens, inside a string, or just after a backslash
/// inside one.
#[derive(Default)]
struct Scan {
    in_string: bool,
    escaped: bool,
}

impl Scan {
    /// Whether the next byte stays: every byte but whitespace between tokens.
    const fn keeps(&mut self, byte: u8) -> bool {
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if byte == b'\\' {
                self.escaped = true;
            } else if byte == b'"' {
                self.in_string = false;
            }
            true
        } else if byte == b'"' {
            self.in_string = true;
            true
        } else {
            !matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
        }
    }
}
