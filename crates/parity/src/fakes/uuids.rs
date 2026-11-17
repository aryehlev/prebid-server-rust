//! The deterministic UUID stream (Go: `parity/fakes/uuids.go`, `fakes.json` `generated_uuids`).
//!
//! Go generates random v4 UUIDs on the request path (the `{{UUID}}` request ID, `source.tid`
//! and `imp[].ext.tid`, generated bid IDs, cache keys). The differ numbers them with
//! placeholders, but a placeholder cannot reach inside a gzip-compressed bidder request, so in
//! parity mode every generated v4 UUID draws from a deterministic stream instead, restarted
//! whenever the fake values are set (at the start and end of every case and on reset).
//!
//! Go replaces `uuid.DefaultGenerator` process-wide. Rust has no such global, so every Rust site
//! that ports a `uuid.NewV4` call uses [`new_v4`], and every injected
//! [`UuidGenerator`] that ports `uuidutil.UUIDRandomGenerator` is wrapped in
//! [`ParityUuidGenerator`].

use pbs_util::uuidutil::{UuidError, UuidGenerator, UuidRandomGenerator, v4_string};
use sha2::{Digest as _, Sha256};

use super::active;

/// The `n`-th UUID of the stream seeded by `seed` (Go: `fakes.SequenceUUID`), `n` counting from 1.
///
/// It is the first 16 bytes of SHA-256 over `seed + ":" + n` in decimal, with the version 4 and
/// RFC 4122 variant bits set, as lowercase `8-4-4-4-12` hex.
#[must_use]
pub fn sequence_uuid(seed: &str, n: u64) -> String {
    let sum = Sha256::new()
        .chain_update(seed.as_bytes())
        .chain_update(b":")
        .chain_update(n.to_string().as_bytes())
        .finalize();
    let mut b = [0_u8; 16];
    for (dst, src) in b.iter_mut().zip(sum.iter()) {
        *dst = *src;
    }
    v4_string(b)
}

/// A generated v4 UUID (Go: `uuid.NewV4` with the parity generator in place): the next UUID of
/// the installed source's stream in parity mode, else a random UUID.
///
/// # Errors
///
/// The random source's error, outside parity mode.
#[inline]
pub fn new_v4() -> Result<String, UuidError> {
    active().map_or_else(|| UuidRandomGenerator.generate(), |src| Ok(src.new_v4()))
}

/// A [`UuidGenerator`] that draws from the deterministic stream while the fakes are installed.
///
/// Otherwise it delegates to the wrapped generator (Go: the `fakes.generator` that
/// `fakes.Install` puts in place of `uuid.DefaultGenerator`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParityUuidGenerator<G>(pub G);

impl<G: UuidGenerator> UuidGenerator for ParityUuidGenerator<G> {
    fn generate(&self) -> Result<String, UuidError> {
        active().map_or_else(|| self.0.generate(), |src| Ok(src.new_v4()))
    }
}
