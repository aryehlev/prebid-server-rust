//! Port of `github.com/coocood/freecache` v1.2.1, the in-memory cache behind Prebid Server's
//! stored request cache (`stored_requests/caches/memory`), the price floors fetcher cache
//! (`floors/fetcher.go`) and the Scope3 RTD module cache.
//!
//! Hits, misses, evictions and expirations decide whether Prebid refetches stored data, so the
//! port reproduces freecache's storage layout and algorithms exactly, not just its contract:
//!
//! - the cache is split into 256 segments chosen by the low byte of the key's XXH64 hash
//!   (`cespare/xxhash/v2` `Sum64`, ported in a private module), each guarded by its own mutex;
//! - each segment is a [`RingBuf`] of `size / 256` bytes (`size` at least 512 KiB) holding 24-byte
//!   entry headers in Go's in-memory layout, keys, values and spare value capacity, indexed by 256
//!   slots of pointers sorted by a 16-bit hash tag;
//! - an entry is rejected with [`CacheError::LargeEntry`] when key plus value exceed `size / 1024 -
//!   24` bytes, and with [`CacheError::LargeKey`] when the key exceeds 65535;
//! - expiration uses `uint32` seconds from a [`Timer`]; `get` treats `expireAt <= now` as expired
//!   while eviction treats `expireAt < now` as expired, as in Go;
//! - when a segment is full, the oldest entry is dropped if deleted or expired, evicted if its
//!   access time is at most the segment average, and otherwise moved to the front, with at most
//!   five moves in a row;
//! - the counters ([`Cache::entry_count`], [`Cache::hit_count`], [`Cache::miss_count`],
//!   [`Cache::evacuate_count`], [`Cache::expired_count`], [`Cache::overwrite_count`],
//!   [`Cache::touched_count`], [`Cache::average_access_time`]) change exactly when Go's do, and can
//!   be read without locking.
//!
//! The same operations on the same clock leave a Rust and a Go cache in byte-identical states.
//! The tests replay long random operation scripts recorded from the Go library by
//! `testdata/go` and compare every result, every counter and digests of every segment's ring
//! buffer and slot tables.
//!
//! # Example
//!
//! ```
//! use pbs_freecache::{Cache, CacheError};
//!
//! let cache = Cache::new(512 * 1024);
//! cache.set(b"account-1", br#"{"id":"1"}"#, 60)?;
//! assert_eq!(cache.get(b"account-1")?, br#"{"id":"1"}"#);
//! assert_eq!(cache.get(b"account-2"), Err(CacheError::NotFound));
//! assert_eq!((cache.hit_count(), cache.miss_count()), (1, 1));
//! # Ok::<(), CacheError>(())
//! ```
//!
//! # Differences from Go
//!
//! - Go's nil and empty slices are both empty vectors. [`Cache::get`] and the other reads that Go
//!   performs into a nil buffer return nil for an empty value, so an empty result there is Go's
//!   nil.
//! - [`Cache::get_fn`] and [`Cache::peek_fn`] return the callback's result instead of propagating a
//!   Go error from it.
//! - `NewCachedTimer` (a timer refreshed by a background goroutine) is not ported; Prebid does not
//!   use it.
//! - Paths on which Go panics or loops forever (impossible given the cache's invariants, or a
//!   zero-sized [`RingBuf`]) do nothing instead.

mod freecache;

pub use freecache::{
    Cache, CacheError, CacheIterator, DefaultTimer, ENTRY_HDR_SIZE, Entry, RingBuf, Timer,
};
