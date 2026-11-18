//! The Go package `github.com/coocood/freecache`, one module per Go file, plus the
//! `cespare/xxhash/v2` `Sum64` it hashes keys with.

mod cache;
mod convert;
mod iterator;
mod ringbuf;
mod segment;
mod timer;
mod xxhash;

pub use cache::Cache;
pub use iterator::{CacheIterator, Entry};
pub use ringbuf::RingBuf;
pub use segment::{CacheError, ENTRY_HDR_SIZE};
pub use timer::{DefaultTimer, Timer};

#[cfg(test)]
mod cache_test;
#[cfg(test)]
mod differential_test;
#[cfg(test)]
mod golden;
#[cfg(test)]
mod ringbuf_test;
