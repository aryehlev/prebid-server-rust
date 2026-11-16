//! Bit stream primitives. Go: package `github.com/prebid/go-gpp/util`.

mod base64;
mod bitstream;
mod encoding;
mod fibonacci;
mod rangesection;

pub use bitstream::{
    BitStream, parse_byte1, parse_byte2, parse_byte4, parse_byte6, parse_byte8, parse_uint12,
    parse_uint16,
};
pub use rangesection::{IRange, IntRange};
