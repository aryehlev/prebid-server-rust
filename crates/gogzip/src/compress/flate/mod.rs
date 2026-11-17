//! Go: `compress/flate`: the DEFLATE format (RFC 1951).
//!
//! [`Writer`] produces the bytes Go's `flate.Writer` produces for the same level, the same
//! sequence of `write` calls and the same flush points. [`Reader`] decompresses with Go's
//! results: the same `(n, err)` pairs for the same buffer sizes and the same errors, such as
//! `flate: corrupt input before offset 5`.
//!
//! The files mirror Go's: `deflate.rs` (the compressor and its levels), `deflatefast.rs`
//! (level 1), `huffman_code.rs`, `huffman_bit_writer.rs`, `token.rs`, `inflate.rs` and
//! `dict_decoder.rs`.

mod deflate;
mod deflatefast;
mod dict_decoder;
#[cfg(test)]
mod golden_test;
mod huffman_bit_writer;
mod huffman_code;
mod inflate;
mod token;

pub(in crate::compress) use deflate::Compressor;
pub use deflate::{
    BEST_COMPRESSION, BEST_SPEED, DEFAULT_COMPRESSION, HUFFMAN_ONLY, NO_COMPRESSION, Writer,
};
pub(in crate::compress) use inflate::Decompressor;
pub use inflate::Reader;
