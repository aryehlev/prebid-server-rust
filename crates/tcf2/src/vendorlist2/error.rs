//! The errors of [`parse_eagerly`](super::parse_eagerly).

use core::fmt;
use std::error::Error;

use crate::base64::CorruptInputError;

/// An error from [`parse_eagerly`](super::parse_eagerly). `Display` gives Go's `err.Error()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorListError {
    /// Go `*json.SyntaxError`: the document is not valid JSON. Holds the message, such as
    /// `invalid character 'x' looking for beginning of value` or `unexpected end of JSON input`.
    Syntax(String),
    /// Go `*json.UnmarshalTypeError`: a JSON value that does not fit the field it is decoded
    /// into.
    UnmarshalType {
        /// The JSON value: `object`, `array`, `string`, `bool`, `number`, or `number <literal>`
        /// for a number out of range for an unsigned integer field.
        value: String,
        /// The struct and field path, such as `vendorListVendorContract.vendors.purposes`;
        /// `None` for the top-level value.
        struct_field: Option<String>,
        /// The Go type, such as `uint16` or `[]uint8`.
        type_name: &'static str,
    },
    /// Go `base64.CorruptInputError`, from a `[]uint8` field given as a string that is not
    /// standard base64.
    Base64(CorruptInputError),
    /// `vendorListVersion` is 0 or missing.
    InvalidVersion,
}

impl fmt::Display for VendorListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => f.write_str(message),
            Self::UnmarshalType {
                value,
                struct_field: Some(field),
                type_name,
            } => write!(
                f,
                "json: cannot unmarshal {value} into Go struct field {field} of type {type_name}"
            ),
            Self::UnmarshalType {
                value,
                struct_field: None,
                type_name,
            } => write!(
                f,
                "json: cannot unmarshal {value} into Go value of type {type_name}"
            ),
            Self::Base64(err) => fmt::Display::fmt(err, f),
            Self::InvalidVersion => {
                f.write_str("data.vendorListVersion was 0 or undefined. Versions should start at 1")
            }
        }
    }
}

impl Error for VendorListError {}
