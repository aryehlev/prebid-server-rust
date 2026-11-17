//! Port of freecache `cache.go`: the cache, split into 256 segments by key hash.

use std::{
    array, fmt,
    sync::{Mutex, MutexGuard, PoisonError, atomic::AtomicI64},
};

#[cfg(test)]
use super::golden::Fnv;
use super::{
    convert::{index, int64_to_float64},
    iterator::CacheIterator,
    segment::{CacheError, SegmentState, SegmentStats, load},
    timer::{DefaultTimer, Timer},
    xxhash,
};

/// Go: `segmentCount`, the number of segments within a cache.
pub(super) const SEGMENT_COUNT: usize = 256;

/// Go: `segmentAndOpVal`, the mask applied to the hash to find the segment.
const SEGMENT_AND_OP_VAL: u64 = 255;

/// Go: `minBufSize`, the smallest cache size.
const MIN_BUF_SIZE: i64 = 512 * 1024;

/// One segment with its lock: Go's `locks[i]` and `segments[i]`.
///
/// The counters live outside the lock because Go reads them with `atomic.LoadInt64` without
/// locking. The alignment keeps neighbouring segments off each other's cache lines.
#[derive(Debug)]
#[repr(align(128))]
pub(super) struct Segment {
    state: Mutex<SegmentState>,
    pub(super) stats: SegmentStats,
}

impl Segment {
    fn new(buf_size: usize) -> Self {
        Self {
            state: Mutex::new(SegmentState::new(buf_size)),
            stats: SegmentStats::default(),
        }
    }

    /// Go: `cache.locks[segID].Lock()`.
    ///
    /// A panic inside a [`Cache::get_fn`] callback poisons the lock without leaving the segment
    /// half-updated (the callback runs after every write), so the state is used as is.
    pub(super) fn lock(&self) -> MutexGuard<'_, SegmentState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Go: `freecache.Cache`, a fixed-size, concurrency-safe byte cache with approximate LRU
/// eviction and per-entry expiration.
///
/// The cache is split into 256 segments, each with its own lock and ring buffer of `size / 256`
/// bytes; a key's segment is the low byte of its XXH64 hash. `T` is the clock; production code
/// uses [`DefaultTimer`] through [`Cache::new`].
pub struct Cache<T = DefaultTimer> {
    segments: Box<[Segment; SEGMENT_COUNT]>,
    timer: T,
}

impl Cache {
    /// Go: `NewCache(size)`, a cache of `size` bytes (at least 512 KiB) on the system clock.
    #[must_use]
    pub fn new(size: i64) -> Self {
        Self::with_timer(size, DefaultTimer)
    }
}

impl<T> Cache<T> {
    /// Go: `NewCacheCustomTimer(size, timer)`, a cache of `size` bytes (at least 512 KiB) on
    /// `timer`.
    #[must_use]
    pub fn with_timer(size: i64, timer: T) -> Self {
        let size = size.max(MIN_BUF_SIZE);
        let buf_size = index(size / 256).unwrap_or(0);
        Self {
            segments: Box::new(array::from_fn(|_| Segment::new(buf_size))),
            timer,
        }
    }

    /// Go: `hashFunc(key)` and `cache.segments[hashVal & segmentAndOpVal]`.
    fn segment(&self, key: &[u8]) -> (&Segment, u64) {
        let hash_val = xxhash::sum64(key);
        let [first, ..] = &*self.segments;
        let seg_id = (hash_val & SEGMENT_AND_OP_VAL) as usize;
        (self.segments.get(seg_id).unwrap_or(first), hash_val)
    }

    /// The segments, for the iterator.
    pub(super) fn segments(&self) -> &[Segment; SEGMENT_COUNT] {
        &self.segments
    }

    /// The clock, for the iterator.
    pub(super) const fn timer(&self) -> &T {
        &self.timer
    }

    /// Go: `Set`, which stores `value` under `key`, expiring after `expire_seconds` seconds.
    ///
    /// `expire_seconds <= 0` means no expiration, but the entry can still be evicted when the
    /// cache is full. Like Go, the seconds are added to the `uint32` clock with wrap-around after
    /// truncation to 32 bits.
    ///
    /// # Errors
    ///
    /// [`CacheError::LargeKey`] for a key longer than 65535 bytes, [`CacheError::LargeEntry`]
    /// when the key and value exceed 1/1024 of the cache size minus the 24-byte header.
    pub fn set(&self, key: &[u8], value: &[u8], expire_seconds: i64) -> Result<(), CacheError>
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        seg.lock().set(
            &seg.stats,
            &self.timer,
            key,
            value,
            hash_val,
            expire_seconds,
        )
    }

    /// Go: `Touch`, which resets the expiration of an existing key. `expire_seconds <= 0` means
    /// no expiration.
    ///
    /// # Errors
    ///
    /// [`CacheError::LargeKey`] for a key longer than 65535 bytes, [`CacheError::NotFound`] for
    /// a missing or expired key.
    pub fn touch(&self, key: &[u8], expire_seconds: i64) -> Result<(), CacheError>
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        seg.lock()
            .touch(&seg.stats, &self.timer, key, hash_val, expire_seconds)
    }

    /// Go: `Get`, the value stored under `key`.
    ///
    /// Go returns a nil slice for an empty value (it reads into a nil buffer), so an empty
    /// vector here is what Go callers see as nil.
    ///
    /// # Errors
    ///
    /// [`CacheError::NotFound`] for a missing or expired key.
    pub fn get(&self, key: &[u8]) -> Result<Vec<u8>, CacheError>
    where
        T: Timer,
    {
        let mut value = Vec::new();
        let (seg, hash_val) = self.segment(key);
        let _ = seg
            .lock()
            .get(&seg.stats, &self.timer, key, &mut value, hash_val, false)?;
        Ok(value)
    }

    /// Go: `GetFn`, which calls `f` with the value in place, holding the segment lock, and
    /// returns its result. The value is copied only when it wraps around the ring buffer.
    ///
    /// Go propagates the error `f` returns; here `f`'s result, which may itself be a `Result`,
    /// is returned as is. The hit is counted after `f` returns.
    ///
    /// # Errors
    ///
    /// [`CacheError::NotFound`] for a missing or expired key; `f` is not called.
    pub fn get_fn<R, F: FnOnce(&[u8]) -> R>(&self, key: &[u8], f: F) -> Result<R, CacheError>
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        seg.lock()
            .view(&seg.stats, &self.timer, key, f, hash_val, false)
    }

    /// Go: `GetOrSet`, which returns the existing value, or stores `value` and returns `None`.
    ///
    /// # Errors
    ///
    /// The [`Cache::set`] error when the key was missing and `value` could not be stored.
    pub fn get_or_set(
        &self,
        key: &[u8],
        value: &[u8],
        expire_seconds: i64,
    ) -> Result<Option<Vec<u8>>, CacheError>
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        let mut state = seg.lock();
        let mut ret_value = Vec::new();
        if state
            .get(
                &seg.stats,
                &self.timer,
                key,
                &mut ret_value,
                hash_val,
                false,
            )
            .is_ok()
        {
            return Ok(Some(ret_value));
        }
        state
            .set(
                &seg.stats,
                &self.timer,
                key,
                value,
                hash_val,
                expire_seconds,
            )
            .map(|()| None)
    }

    /// Go: `SetAndGet`, which stores `value` and returns the previous value, if any (Go's
    /// `found`), with the result of storing.
    pub fn set_and_get(
        &self,
        key: &[u8],
        value: &[u8],
        expire_seconds: i64,
    ) -> (Option<Vec<u8>>, Result<(), CacheError>)
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        let mut state = seg.lock();
        let mut ret_value = Vec::new();
        let found = state
            .get(
                &seg.stats,
                &self.timer,
                key,
                &mut ret_value,
                hash_val,
                false,
            )
            .is_ok();
        let result = state.set(
            &seg.stats,
            &self.timer,
            key,
            value,
            hash_val,
            expire_seconds,
        );
        drop(state);
        (found.then_some(ret_value), result)
    }

    /// Go: `Peek`, the value under `key` without updating its access time or the counters.
    ///
    /// Like Go, it does not check expiration: an expired entry that is still stored is returned.
    ///
    /// # Errors
    ///
    /// [`CacheError::NotFound`] for a missing key.
    pub fn peek(&self, key: &[u8]) -> Result<Vec<u8>, CacheError>
    where
        T: Timer,
    {
        let mut value = Vec::new();
        let (seg, hash_val) = self.segment(key);
        let _ = seg
            .lock()
            .get(&seg.stats, &self.timer, key, &mut value, hash_val, true)?;
        Ok(value)
    }

    /// Go: `PeekFn`, [`Cache::get_fn`] without updating the access time or the counters, and
    /// without checking expiration.
    ///
    /// # Errors
    ///
    /// [`CacheError::NotFound`] for a missing key; `f` is not called.
    pub fn peek_fn<R, F: FnOnce(&[u8]) -> R>(&self, key: &[u8], f: F) -> Result<R, CacheError>
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        seg.lock()
            .view(&seg.stats, &self.timer, key, f, hash_val, true)
    }

    /// Go: `GetWithBuf`, which copies the value into `buf`, reusing its allocation when its
    /// capacity suffices. `buf` is left untouched on a miss.
    ///
    /// # Errors
    ///
    /// [`CacheError::NotFound`] for a missing or expired key.
    pub fn get_with_buf(&self, key: &[u8], buf: &mut Vec<u8>) -> Result<(), CacheError>
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        seg.lock()
            .get(&seg.stats, &self.timer, key, buf, hash_val, false)
            .map(|_| ())
    }

    /// Go: `GetWithExpiration`, the value and its expiration time (`0` for none).
    ///
    /// # Errors
    ///
    /// [`CacheError::NotFound`] for a missing or expired key.
    pub fn get_with_expiration(&self, key: &[u8]) -> Result<(Vec<u8>, u32), CacheError>
    where
        T: Timer,
    {
        let mut value = Vec::new();
        let (seg, hash_val) = self.segment(key);
        let expire_at =
            seg.lock()
                .get(&seg.stats, &self.timer, key, &mut value, hash_val, false)?;
        Ok((value, expire_at))
    }

    /// Go: `TTL`, the seconds left before `key` expires, `0` when it does not expire.
    ///
    /// # Errors
    ///
    /// [`CacheError::NotFound`] for a missing or expired key.
    pub fn ttl(&self, key: &[u8]) -> Result<u32, CacheError>
    where
        T: Timer,
    {
        let (seg, hash_val) = self.segment(key);
        seg.lock().ttl(&self.timer, key, hash_val)
    }

    /// Go: `Del`, which deletes `key` and reports whether it was stored.
    pub fn del(&self, key: &[u8]) -> bool {
        let (seg, hash_val) = self.segment(key);
        seg.lock().del(&seg.stats, key, hash_val)
    }

    /// Go: `SetInt`, [`Cache::set`] keyed by the 8 little-endian bytes of `key`.
    ///
    /// # Errors
    ///
    /// As [`Cache::set`].
    pub fn set_int(&self, key: i64, value: &[u8], expire_seconds: i64) -> Result<(), CacheError>
    where
        T: Timer,
    {
        self.set(&key.to_le_bytes(), value, expire_seconds)
    }

    /// Go: `GetInt`, [`Cache::get`] keyed by the 8 little-endian bytes of `key`.
    ///
    /// # Errors
    ///
    /// As [`Cache::get`].
    pub fn get_int(&self, key: i64) -> Result<Vec<u8>, CacheError>
    where
        T: Timer,
    {
        self.get(&key.to_le_bytes())
    }

    /// Go: `GetIntWithExpiration`, [`Cache::get_with_expiration`] keyed by the 8 little-endian
    /// bytes of `key`.
    ///
    /// # Errors
    ///
    /// As [`Cache::get_with_expiration`].
    pub fn get_int_with_expiration(&self, key: i64) -> Result<(Vec<u8>, u32), CacheError>
    where
        T: Timer,
    {
        self.get_with_expiration(&key.to_le_bytes())
    }

    /// Go: `DelInt`, [`Cache::del`] keyed by the 8 little-endian bytes of `key`.
    pub fn del_int(&self, key: i64) -> bool {
        self.del(&key.to_le_bytes())
    }

    /// Sums one counter over the segments, wrapping like Go's `int64` addition.
    fn sum(&self, counter: fn(&SegmentStats) -> &AtomicI64) -> i64 {
        self.segments.iter().fold(0, |total, seg| {
            total.wrapping_add(load(counter(&seg.stats)))
        })
    }

    /// Go: `EvacuateCount`, the number of entries evicted or moved by evacuation.
    #[must_use]
    pub fn evacuate_count(&self) -> i64 {
        self.sum(|stats| &stats.total_evacuate)
    }

    /// Go: `ExpiredCount`, the number of entries removed because they expired.
    #[must_use]
    pub fn expired_count(&self) -> i64 {
        self.sum(|stats| &stats.total_expired)
    }

    /// Go: `EntryCount`, the number of entries currently in the cache.
    #[must_use]
    pub fn entry_count(&self) -> i64 {
        self.sum(|stats| &stats.entry_count)
    }

    /// Go: `AverageAccessTime`, the average access time, in Unix seconds, of the entries in the
    /// ring buffers (deleted ones included), or `0` when there are none.
    #[must_use]
    pub fn average_access_time(&self) -> i64 {
        let total_time = self.sum(|stats| &stats.total_time);
        let entry_count = self.sum(|stats| &stats.total_count);
        if entry_count == 0 {
            0
        } else {
            total_time.wrapping_div(entry_count)
        }
    }

    /// Go: `HitCount`, the number of lookups that found their key.
    #[must_use]
    pub fn hit_count(&self) -> i64 {
        self.sum(|stats| &stats.hit_count)
    }

    /// Go: `MissCount`, the number of lookups that missed.
    #[must_use]
    pub fn miss_count(&self) -> i64 {
        self.sum(|stats| &stats.miss_count)
    }

    /// Go: `LookupCount`, hits plus misses.
    #[must_use]
    pub fn lookup_count(&self) -> i64 {
        self.hit_count().wrapping_add(self.miss_count())
    }

    /// Go: `HitRate`, hits over lookups, or `0` without lookups.
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        let hit_count = self.hit_count();
        let miss_count = self.miss_count();
        let lookup_count = hit_count.wrapping_add(miss_count);
        if lookup_count == 0 {
            0.0
        } else {
            int64_to_float64(hit_count) / int64_to_float64(lookup_count)
        }
    }

    /// Go: `OverwriteCount`, the number of values overwritten in place.
    #[must_use]
    pub fn overwrite_count(&self) -> i64 {
        self.sum(|stats| &stats.overwrites)
    }

    /// Go: `TouchedCount`, the number of successful [`Cache::touch`] calls.
    #[must_use]
    pub fn touched_count(&self) -> i64 {
        self.sum(|stats| &stats.touched)
    }

    /// Go: `Clear`, which empties every segment and resets its counters (not the touched count).
    pub fn clear(&self) {
        for seg in &*self.segments {
            seg.lock().clear(&seg.stats);
        }
    }

    /// Go: `ResetStatistics`, which zeroes the evacuate, expired, overwrite, hit and miss counts.
    pub fn reset_statistics(&self) {
        for seg in &*self.segments {
            let guard = seg.lock();
            SegmentState::reset_statistics(&seg.stats);
            drop(guard);
        }
    }

    /// Go: `NewIterator`, an iterator over the entries that have not expired.
    #[must_use]
    pub const fn new_iterator(&self) -> CacheIterator<'_, T> {
        CacheIterator::new(self)
    }

    /// Feeds every segment's state to a parity digest, in segment order.
    #[cfg(test)]
    pub(super) fn digest_state(&self, digest: &mut Fnv) {
        for seg in &*self.segments {
            seg.lock().digest(&seg.stats, digest);
        }
    }
}

impl<T> fmt::Debug for Cache<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cache")
            .field("entry_count", &self.entry_count())
            .field("hit_count", &self.hit_count())
            .field("miss_count", &self.miss_count())
            .finish_non_exhaustive()
    }
}
