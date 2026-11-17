//! Go `encoding/json` and json-iterator compatible JSON encoding and decoding for Prebid Server.

#[doc(hidden)]
pub mod base64;
pub mod custom;
pub mod encode;
mod error;
mod gostring;
pub mod gotype;
pub mod jsoniter;
pub mod jsonutil;
mod number;
mod raw;
pub mod stdjson;
#[doc(hidden)]
pub mod strconv;
pub mod structs;
#[doc(hidden)]
pub mod unicode;

#[doc(hidden)]
pub mod __private;

pub use custom::{GoMarshaler, GoUnmarshaler};
pub use encode::{Encoder, Flavor, GoEncode, GoMapKey};
pub use error::JsonError;
pub use gostring::GoString;
pub use gotype::{GoKind, GoType};
pub use jsoniter::JsoniterDecode;
pub use number::JsonNumber;
pub use pbs_gojson_derive::GoJson;
pub use raw::RawJson;
pub use stdjson::StdDecode;
