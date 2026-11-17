//! The error type shared by every encoder and decoder in this crate.

use core::{error::Error, fmt};

use crate::unicode::go_lossy;

/// An error produced by a Go-compatible encode or decode, carrying the exact text Go's `error`
/// value would print.
///
/// Go error strings are byte strings and can contain invalid UTF-8 (jsoniter quotes raw input
/// bytes in its messages); [`JsonError::go_text`] returns them exactly, while `Display` renders
/// them with every invalid byte replaced by U+FFFD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonError {
    /// `encoding/json`'s `*SyntaxError` (also returned by `json.Compact` and `json.Valid`-style
    /// checks).
    Syntax {
        /// The description, e.g. `invalid character 'x' looking for beginning of value`.
        msg: String,
        /// Bytes read before the error, as Go's `SyntaxError.Offset`.
        offset: i64,
    },
    /// `encoding/json`'s `*UnmarshalTypeError`.
    UnmarshalType {
        /// The JSON value description: `string`, `number`, `number 1.5`, `bool`, `array`,
        /// `object`.
        value: String,
        /// Go's `reflect.Type.String()` of the destination.
        go_type: String,
        /// Best-effort input offset (Go's `Offset`).
        offset: i64,
        /// Name of the struct holding the field (`reflect.Type.Name()`, no package).
        struct_name: String,
        /// Dotted path of JSON keys (and embedded Go field names) leading to the field.
        field: String,
    },
    /// Any other error, as the exact bytes of Go's `Error()`.
    Message(Vec<u8>),
}

impl JsonError {
    /// An error whose `Error()` text is `text`.
    #[must_use]
    pub fn msg<T: Into<Vec<u8>>>(text: T) -> Self {
        Self::Message(text.into())
    }

    /// The exact bytes Go's `err.Error()` returns.
    #[must_use]
    pub fn go_text(&self) -> Vec<u8> {
        match self {
            Self::Syntax { msg, .. } => msg.as_bytes().to_vec(),
            Self::UnmarshalType {
                value,
                go_type,
                struct_name,
                field,
                ..
            } => {
                // Go: encoding/json/decode.go (*UnmarshalTypeError).Error
                let text = if struct_name.is_empty() && field.is_empty() {
                    format!("json: cannot unmarshal {value} into Go value of type {go_type}")
                } else {
                    format!(
                        "json: cannot unmarshal {value} into Go struct field \
                         {struct_name}.{field} of type {go_type}"
                    )
                };
                text.into_bytes()
            }
            Self::Message(bytes) => bytes.clone(),
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&go_lossy(&self.go_text()))
    }
}

impl Error for JsonError {}
