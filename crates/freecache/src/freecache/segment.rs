//! Port of freecache `segment.go`: one of the 256 independently locked parts of a cache.
//!
//! A segment stores entries in a [`RingBuf`]: a 24-byte header, the key, then the value
//! followed by its unused capacity. 256 slots of entry pointers, each sorted by the 16-bit hash
//! tag, index the ring. When the ring is full, [`SegmentState::evacuate`] reclaims the oldest
//! entry: deleted and expired entries are dropped, entries accessed no later than the segment's
//! average access time are evicted, and the others are moved to the front of the ring, up to
//! six in a row (Go's `consecutiveEvacuate > 5`) before the next one is evicted regardless.

use std::{
    error, fmt,
    sync::atomic::{AtomicI64, Ordering},
};

#[cfg(test)]
use super::golden::Fnv;
use super::{
    convert::{copy, low_u32, wide},
    ringbuf::RingBuf,
    timer::Timer,
};

/// Go: `ENTRY_HDR_SIZE`, the size of the header stored before each entry's key and value.
pub const ENTRY_HDR_SIZE: usize = 24;

/// [`ENTRY_HDR_SIZE`] as the Go `int64` the segment arithmetic uses.
pub(super) const HDR_SIZE: i64 = 24;

/// Go: the `65535` key length limit.
const MAX_KEY_LEN: usize = 65535;

/// Go: the number of slots per segment.
const SLOT_COUNT: usize = 256;

/// The errors freecache returns, with Go's texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheError {
    /// Go: `ErrLargeKey`, a key longer than 65535 bytes.
    LargeKey,
    /// Go: `ErrLargeEntry`, a key and value longer than 1/1024 of the cache size minus the
    /// entry header.
    LargeEntry,
    /// Go: `ErrNotFound`, a missing or expired key.
    NotFound,
    /// Go: `ErrOutOfRange`, a [`RingBuf`] access outside the stream.
    OutOfRange,
    /// Go: `io.EOF`, a [`RingBuf`] read past the end of the stream.
    Eof,
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LargeKey => "The key is larger than 65535",
            Self::LargeEntry => "The entry size is larger than 1/1024 of cache size",
            Self::NotFound => "Entry not found",
            Self::OutOfRange => "out of range",
            Self::Eof => "EOF",
        })
    }
}

impl error::Error for CacheError {}

/// Go: `entryPtr`, which points to an entry in the ring buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EntryPtr {
    /// Go: `offset`, the entry's offset in the ring buffer stream.
    pub(super) offset: i64,
    /// Go: `hash16`; a slot's pointers are ordered by it.
    pub(super) hash16: u16,
    /// Go: `keyLen`, used to compare a key.
    pub(super) key_len: u16,
}

/// Go: `entryHdr`, the header stored in the ring buffer before each key and value.
///
/// Go reinterprets the 24 bytes in place; on the little-endian platforms Prebid runs on that is
/// this field order, little endian.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EntryHdr {
    pub(super) access_time: u32,
    pub(super) expire_at: u32,
    pub(super) key_len: u16,
    pub(super) hash16: u16,
    pub(super) val_len: u32,
    pub(super) val_cap: u32,
    pub(super) deleted: bool,
    pub(super) slot_id: u8,
    pub(super) reserved: u16,
}

impl EntryHdr {
    /// Reads a header from its 24 stored bytes.
    const fn decode(bytes: [u8; ENTRY_HDR_SIZE]) -> Self {
        let [
            a0,
            a1,
            a2,
            a3,
            e0,
            e1,
            e2,
            e3,
            k0,
            k1,
            h0,
            h1,
            l0,
            l1,
            l2,
            l3,
            c0,
            c1,
            c2,
            c3,
            deleted,
            slot_id,
            r0,
            r1,
        ] = bytes;
        Self {
            access_time: u32::from_le_bytes([a0, a1, a2, a3]),
            expire_at: u32::from_le_bytes([e0, e1, e2, e3]),
            key_len: u16::from_le_bytes([k0, k1]),
            hash16: u16::from_le_bytes([h0, h1]),
            val_len: u32::from_le_bytes([l0, l1, l2, l3]),
            val_cap: u32::from_le_bytes([c0, c1, c2, c3]),
            deleted: deleted != 0,
            slot_id,
            reserved: u16::from_le_bytes([r0, r1]),
        }
    }

    /// The 24 bytes Go stores for this header.
    fn encode(&self) -> [u8; ENTRY_HDR_SIZE] {
        let [a0, a1, a2, a3] = self.access_time.to_le_bytes();
        let [e0, e1, e2, e3] = self.expire_at.to_le_bytes();
        let [k0, k1] = self.key_len.to_le_bytes();
        let [h0, h1] = self.hash16.to_le_bytes();
        let [l0, l1, l2, l3] = self.val_len.to_le_bytes();
        let [c0, c1, c2, c3] = self.val_cap.to_le_bytes();
        let [r0, r1] = self.reserved.to_le_bytes();
        [
            a0,
            a1,
            a2,
            a3,
            e0,
            e1,
            e2,
            e3,
            k0,
            k1,
            h0,
            h1,
            l0,
            l1,
            l2,
            l3,
            c0,
            c1,
            c2,
            c3,
            u8::from(self.deleted),
            self.slot_id,
            r0,
            r1,
        ]
    }
}

/// The counters of a segment, which Go reads with `atomic.LoadInt64` without taking the lock.
#[derive(Debug, Default)]
pub(super) struct SegmentStats {
    pub(super) miss_count: AtomicI64,
    pub(super) hit_count: AtomicI64,
    pub(super) entry_count: AtomicI64,
    /// Go: `totalCount`, the number of entries in the ring buffer, deleted ones included.
    pub(super) total_count: AtomicI64,
    /// Go: `totalTime`, the sum of the access times of those entries.
    pub(super) total_time: AtomicI64,
    pub(super) total_evacuate: AtomicI64,
    pub(super) total_expired: AtomicI64,
    pub(super) overwrites: AtomicI64,
    pub(super) touched: AtomicI64,
}

/// Go: `atomic.LoadInt64`.
pub(super) fn load(counter: &AtomicI64) -> i64 {
    counter.load(Ordering::Relaxed)
}

/// Go: `atomic.AddInt64`. Every writer holds the segment lock, so a load and a store add
/// atomically; lock-free readers see the old or the new value.
fn add(counter: &AtomicI64, delta: i64) {
    counter.store(load(counter).wrapping_add(delta), Ordering::Relaxed);
}

/// Go: `atomic.StoreInt64(counter, 0)`.
fn zero(counter: &AtomicI64) {
    counter.store(0, Ordering::Relaxed);
}

/// Go: `uint8(hashVal >> 8)` and `uint16(hashVal >> 16)`.
const fn slot_and_hash16(hash_val: u64) -> (u8, u16) {
    (
        ((hash_val >> 8) & 0xFF) as u8,
        ((hash_val >> 16) & 0xFFFF) as u16,
    )
}

/// Go: `uint16(len(key))`, `uint32(len(value))`.
fn key_len16(key: &[u8]) -> u16 {
    u16::try_from(key.len() & 0xFFFF).unwrap_or(u16::MAX)
}

/// Go: the expiration time `set` and `touch` compute.
const fn expiration(now: u32, expire_seconds: i64) -> u32 {
    if expire_seconds > 0 {
        now.wrapping_add(low_u32(expire_seconds))
    } else {
        0
    }
}

/// Go: `segment`, the part of a segment that its lock guards.
#[derive(Debug)]
pub(super) struct SegmentState {
    /// Go: `rb`, the ring buffer that stores data.
    rb: RingBuf,
    /// Go: `vacuumLen`; up to it, new data can be written without overwriting old data.
    vacuum_len: i64,
    /// Go: `slotLens`, the actual length of every slot.
    slot_lens: Vec<usize>,
    /// Go: `slotCap`, the maximum number of entry pointers a slot can hold.
    slot_cap: usize,
    /// Go: `slotsData`, shared by all 256 slots.
    slots_data: Vec<EntryPtr>,
}

impl SegmentState {
    /// Go: `newSegment(bufSize, segId, timer)`.
    pub(super) fn new(buf_size: usize) -> Self {
        Self {
            rb: RingBuf::new(buf_size, 0),
            vacuum_len: wide(buf_size),
            slot_lens: vec![0; SLOT_COUNT],
            slot_cap: 1,
            slots_data: vec![EntryPtr::default(); SLOT_COUNT],
        }
    }

    /// Go: `set`.
    pub(super) fn set<T: Timer + ?Sized>(
        &mut self,
        stats: &SegmentStats,
        timer: &T,
        key: &[u8],
        value: &[u8],
        hash_val: u64,
        expire_seconds: i64,
    ) -> Result<(), CacheError> {
        if key.len() > MAX_KEY_LEN {
            return Err(CacheError::LargeKey);
        }
        let max_key_val_len = self.rb.size() / 4 - HDR_SIZE;
        if wide(key.len()) + wide(value.len()) > max_key_val_len {
            // Do not accept large entry.
            return Err(CacheError::LargeEntry);
        }
        let now = timer.now();
        let expire_at = expiration(now, expire_seconds);

        let (slot_id, hash16) = slot_and_hash16(hash_val);
        let (mut idx, matched) = self.lookup(slot_id, hash16, key);

        let mut hdr;
        if matched {
            let matched_off = self.ptr_offset(slot_id, idx);
            hdr = self.read_hdr(matched_off);
            hdr.slot_id = slot_id;
            hdr.hash16 = hash16;
            hdr.key_len = key_len16(key);
            let origin_access_time = hdr.access_time;
            hdr.access_time = now;
            hdr.expire_at = expire_at;
            hdr.val_len = low_u32(wide(value.len()));
            if hdr.val_cap >= hdr.val_len {
                // in place overwrite
                add(
                    &stats.total_time,
                    i64::from(hdr.access_time) - i64::from(origin_access_time),
                );
                self.write_hdr(&hdr, matched_off);
                let _ = self
                    .rb
                    .write_at(value, matched_off + HDR_SIZE + i64::from(hdr.key_len));
                add(&stats.overwrites, 1);
                return Ok(());
            }
            // avoid unnecessary memory copy.
            self.del_entry_ptr(stats, slot_id, idx);
            // increase capacity and limit entry len. A zero capacity, which Go would double
            // forever, cannot be stored.
            while hdr.val_cap < hdr.val_len && hdr.val_cap != 0 {
                hdr.val_cap = hdr.val_cap.wrapping_mul(2);
            }
            let limit = low_u32(max_key_val_len - wide(key.len()));
            if hdr.val_cap > limit {
                hdr.val_cap = limit;
            }
        } else {
            let val_len = low_u32(wide(value.len()));
            hdr = EntryHdr {
                slot_id,
                hash16,
                key_len: key_len16(key),
                access_time: now,
                expire_at,
                val_len,
                // avoid infinite loop when increasing capacity.
                val_cap: if val_len == 0 { 1 } else { val_len },
                ..EntryHdr::default()
            };
        }

        let entry_len = HDR_SIZE + wide(key.len()) + i64::from(hdr.val_cap);
        let slot_modified = self.evacuate(stats, entry_len, slot_id, now);
        if slot_modified {
            // the slot has been modified during evacuation, we need to looked up for the 'idx'
            // again.
            (idx, _) = self.lookup(slot_id, hash16, key);
        }
        let new_off = self.rb.end();
        self.insert_entry_ptr(stats, slot_id, hash16, new_off, idx, hdr.key_len);
        let _ = self.rb.write(&hdr.encode());
        let _ = self.rb.write(key);
        let _ = self.rb.write(value);
        self.rb
            .skip(i64::from(hdr.val_cap.wrapping_sub(hdr.val_len)));
        add(&stats.total_time, i64::from(now));
        add(&stats.total_count, 1);
        self.vacuum_len -= entry_len;
        Ok(())
    }

    /// Go: `touch`.
    pub(super) fn touch<T: Timer + ?Sized>(
        &mut self,
        stats: &SegmentStats,
        timer: &T,
        key: &[u8],
        hash_val: u64,
        expire_seconds: i64,
    ) -> Result<(), CacheError> {
        if key.len() > MAX_KEY_LEN {
            return Err(CacheError::LargeKey);
        }

        let (slot_id, hash16) = slot_and_hash16(hash_val);
        let (idx, matched) = self.lookup(slot_id, hash16, key);
        if !matched {
            return Err(CacheError::NotFound);
        }
        let matched_off = self.ptr_offset(slot_id, idx);
        let mut hdr = self.read_hdr(matched_off);

        let now = timer.now();
        if hdr.expire_at != 0 && hdr.expire_at <= now {
            self.del_entry_ptr(stats, slot_id, idx);
            add(&stats.total_expired, 1);
            add(&stats.miss_count, 1);
            return Err(CacheError::NotFound);
        }

        let expire_at = expiration(now, expire_seconds);
        let origin_access_time = hdr.access_time;
        hdr.access_time = now;
        hdr.expire_at = expire_at;
        // in place overwrite
        add(
            &stats.total_time,
            i64::from(hdr.access_time) - i64::from(origin_access_time),
        );
        self.write_hdr(&hdr, matched_off);
        add(&stats.touched, 1);
        Ok(())
    }

    /// Go: `evacuate`, which frees ring space until `entry_len` bytes fit and reports whether a
    /// pointer of slot `slot_id` was removed.
    fn evacuate(&mut self, stats: &SegmentStats, entry_len: i64, slot_id: u8, now: u32) -> bool {
        let mut slot_modified = false;
        let mut consecutive_evacuate = 0;
        while self.vacuum_len < entry_len {
            let old_off = self.rb.end() + self.vacuum_len - self.rb.size();
            let old_hdr = self.read_hdr(old_off);
            let old_entry_len = HDR_SIZE + i64::from(old_hdr.key_len) + i64::from(old_hdr.val_cap);
            if old_hdr.deleted {
                consecutive_evacuate = 0;
                add(&stats.total_time, -i64::from(old_hdr.access_time));
                add(&stats.total_count, -1);
                self.vacuum_len += old_entry_len;
                continue;
            }
            let expired = old_hdr.expire_at != 0 && old_hdr.expire_at < now;
            let least_recent_used = i64::from(old_hdr.access_time)
                .wrapping_mul(load(&stats.total_count))
                <= load(&stats.total_time);
            if expired || least_recent_used || consecutive_evacuate > 5 {
                self.del_entry_ptr_by_offset(stats, old_hdr.slot_id, old_hdr.hash16, old_off);
                if old_hdr.slot_id == slot_id {
                    slot_modified = true;
                }
                consecutive_evacuate = 0;
                add(&stats.total_time, -i64::from(old_hdr.access_time));
                add(&stats.total_count, -1);
                self.vacuum_len += old_entry_len;
                if expired {
                    add(&stats.total_expired, 1);
                } else {
                    add(&stats.total_evacuate, 1);
                }
            } else {
                // evacuate an old entry that has been accessed recently for better cache hit
                // rate.
                let new_off = self.rb.evacuate(old_off, old_entry_len);
                self.update_entry_ptr(old_hdr.slot_id, old_hdr.hash16, old_off, new_off);
                consecutive_evacuate += 1;
                add(&stats.total_evacuate, 1);
            }
        }
        slot_modified
    }

    /// Go: `get`, copying the value into `out` (cleared first) and returning its expiration.
    ///
    /// `out` is left untouched on a miss.
    pub(super) fn get<T: Timer + ?Sized>(
        &mut self,
        stats: &SegmentStats,
        timer: &T,
        key: &[u8],
        out: &mut Vec<u8>,
        hash_val: u64,
        peek: bool,
    ) -> Result<u32, CacheError> {
        let (hdr, offset) = self.locate(stats, timer, key, hash_val, peek)?;
        out.clear();
        out.resize(usize::try_from(hdr.val_len).unwrap_or(0), 0);
        let _ = self
            .rb
            .read_at(out, offset + HDR_SIZE + i64::from(hdr.key_len));
        if !peek {
            add(&stats.hit_count, 1);
        }
        Ok(hdr.expire_at)
    }

    /// Go: `view`, which calls `f` with the value in place (copied only when it wraps around
    /// the ring buffer).
    pub(super) fn view<T: Timer + ?Sized, R, F: FnOnce(&[u8]) -> R>(
        &mut self,
        stats: &SegmentStats,
        timer: &T,
        key: &[u8],
        f: F,
        hash_val: u64,
        peek: bool,
    ) -> Result<R, CacheError> {
        let (hdr, offset) = self.locate(stats, timer, key, hash_val, peek)?;
        let start = offset + HDR_SIZE + i64::from(hdr.key_len);
        let val = self.rb.slice(start, i64::from(hdr.val_len))?;
        let result = f(&val);
        if !peek {
            add(&stats.hit_count, 1);
        }
        Ok(result)
    }

    /// Go: `locate`, which finds a live entry and, unless peeking, expires it or stamps its
    /// access time. Returns the header and the entry offset.
    fn locate<T: Timer + ?Sized>(
        &mut self,
        stats: &SegmentStats,
        timer: &T,
        key: &[u8],
        hash_val: u64,
        peek: bool,
    ) -> Result<(EntryHdr, i64), CacheError> {
        let (slot_id, hash16) = slot_and_hash16(hash_val);
        let (idx, matched) = self.lookup(slot_id, hash16, key);
        if !matched {
            if !peek {
                add(&stats.miss_count, 1);
            }
            return Err(CacheError::NotFound);
        }
        let offset = self.ptr_offset(slot_id, idx);
        let mut hdr = self.read_hdr(offset);
        if !peek {
            let now = timer.now();
            if hdr.expire_at != 0 && hdr.expire_at <= now {
                self.del_entry_ptr(stats, slot_id, idx);
                add(&stats.total_expired, 1);
                add(&stats.miss_count, 1);
                return Err(CacheError::NotFound);
            }
            // Go: `int64(now-hdr.accessTime)`, a uint32 difference that wraps when the clock
            // went back.
            add(
                &stats.total_time,
                i64::from(now.wrapping_sub(hdr.access_time)),
            );
            hdr.access_time = now;
            self.write_hdr(&hdr, offset);
        }
        Ok((hdr, offset))
    }

    /// Go: `del`.
    pub(super) fn del(&mut self, stats: &SegmentStats, key: &[u8], hash_val: u64) -> bool {
        let (slot_id, hash16) = slot_and_hash16(hash_val);
        let (idx, matched) = self.lookup(slot_id, hash16, key);
        if !matched {
            return false;
        }
        self.del_entry_ptr(stats, slot_id, idx);
        true
    }

    /// Go: `ttl`.
    pub(super) fn ttl<T: Timer + ?Sized>(
        &self,
        timer: &T,
        key: &[u8],
        hash_val: u64,
    ) -> Result<u32, CacheError> {
        let (slot_id, hash16) = slot_and_hash16(hash_val);
        let (idx, matched) = self.lookup(slot_id, hash16, key);
        if !matched {
            return Err(CacheError::NotFound);
        }
        let offset = self.ptr_offset(slot_id, idx);
        let now = timer.now();

        let hdr = self.read_hdr(offset);

        if hdr.expire_at == 0 {
            return Ok(0);
        } else if hdr.expire_at >= now {
            return Ok(hdr.expire_at - now);
        }
        Err(CacheError::NotFound)
    }

    /// Go: `expand`, doubling the capacity of every slot.
    fn expand(&mut self) {
        let mut new_slot_data = vec![EntryPtr::default(); self.slot_cap * 2 * SLOT_COUNT];
        for (i, &len) in self.slot_lens.iter().enumerate() {
            let off = i * self.slot_cap;
            let _ = copy(
                new_slot_data.get_mut(off * 2..).unwrap_or_default(),
                self.slots_data.get(off..off + len).unwrap_or_default(),
            );
        }
        self.slot_cap *= 2;
        self.slots_data = new_slot_data;
    }

    /// Go: `updateEntryPtr`.
    fn update_entry_ptr(&mut self, slot_id: u8, hash16: u16, old_off: i64, new_off: i64) {
        let (idx, matched) = self.lookup_by_off(slot_id, hash16, old_off);
        if !matched {
            return;
        }
        if let Some(ptr) = self.slot_mut(slot_id).get_mut(idx) {
            ptr.offset = new_off;
        }
    }

    /// Go: `insertEntryPtr`.
    fn insert_entry_ptr(
        &mut self,
        stats: &SegmentStats,
        slot_id: u8,
        hash16: u16,
        offset: i64,
        idx: usize,
        key_len: u16,
    ) {
        if self.slot_len(slot_id) == self.slot_cap {
            self.expand();
        }
        if let Some(len) = self.slot_lens.get_mut(usize::from(slot_id)) {
            *len += 1;
        }
        add(&stats.entry_count, 1);
        let slot = self.slot_mut(slot_id);
        // Go: `copy(slot[idx+1:], slot[idx:])`; the last element is the unused one past the
        // old length.
        if let Some(tail) = slot.get_mut(idx..)
            && !tail.is_empty()
        {
            tail.rotate_right(1);
        }
        if let Some(ptr) = slot.get_mut(idx) {
            ptr.offset = offset;
            ptr.hash16 = hash16;
            ptr.key_len = key_len;
        }
    }

    /// Go: `delEntryPtrByOffset`.
    fn del_entry_ptr_by_offset(
        &mut self,
        stats: &SegmentStats,
        slot_id: u8,
        hash16: u16,
        offset: i64,
    ) {
        let (idx, matched) = self.lookup_by_off(slot_id, hash16, offset);
        if !matched {
            return;
        }
        self.del_entry_ptr(stats, slot_id, idx);
    }

    /// Go: `delEntryPtr`, which marks the entry deleted in the ring and removes its pointer.
    fn del_entry_ptr(&mut self, stats: &SegmentStats, slot_id: u8, idx: usize) {
        let Some(offset) = self.slot(slot_id).get(idx).map(|ptr| ptr.offset) else {
            // Unreachable: callers pass the index of a live pointer. Go would panic.
            return;
        };
        let mut entry_hdr = self.read_hdr(offset);
        entry_hdr.deleted = true;
        self.write_hdr(&entry_hdr, offset);
        // Go: `copy(slot[idx:], slot[idx+1:])`.
        if let Some(tail) = self.slot_mut(slot_id).get_mut(idx..)
            && !tail.is_empty()
        {
            tail.rotate_left(1);
        }
        if let Some(len) = self.slot_lens.get_mut(usize::from(slot_id)) {
            *len = len.wrapping_sub(1);
        }
        add(&stats.entry_count, -1);
    }

    /// Go: `lookup`, the index of `key` in slot `slot_id`, or where it would be inserted.
    fn lookup(&self, slot_id: u8, hash16: u16, key: &[u8]) -> (usize, bool) {
        let slot = self.slot(slot_id);
        let mut idx = entry_ptr_idx(slot, hash16);
        while let Some(ptr) = slot.get(idx) {
            if ptr.hash16 != hash16 {
                break;
            }
            if usize::from(ptr.key_len) == key.len() && self.rb.equal_at(key, ptr.offset + HDR_SIZE)
            {
                return (idx, true);
            }
            idx += 1;
        }
        (idx, false)
    }

    /// Go: `lookupByOff`.
    fn lookup_by_off(&self, slot_id: u8, hash16: u16, offset: i64) -> (usize, bool) {
        let slot = self.slot(slot_id);
        let mut idx = entry_ptr_idx(slot, hash16);
        while let Some(ptr) = slot.get(idx) {
            if ptr.hash16 != hash16 {
                break;
            }
            if ptr.offset == offset {
                return (idx, true);
            }
            idx += 1;
        }
        (idx, false)
    }

    /// Go: `resetStatistics`.
    pub(super) fn reset_statistics(stats: &SegmentStats) {
        zero(&stats.total_evacuate);
        zero(&stats.total_expired);
        zero(&stats.overwrites);
        zero(&stats.hit_count);
        zero(&stats.miss_count);
    }

    /// Go: `clear`. Like Go, it leaves the touched count alone.
    pub(super) fn clear(&mut self, stats: &SegmentStats) {
        let buf_size = self.rb.size();
        self.rb.reset(0);
        self.vacuum_len = buf_size;
        self.slot_cap = 1;
        self.slots_data = vec![EntryPtr::default(); SLOT_COUNT * self.slot_cap];
        self.slot_lens.fill(0);

        zero(&stats.hit_count);
        zero(&stats.miss_count);
        zero(&stats.entry_count);
        zero(&stats.total_count);
        zero(&stats.total_time);
        zero(&stats.total_evacuate);
        zero(&stats.total_expired);
        zero(&stats.overwrites);
    }

    /// Go: `slotLens[slotId]`.
    fn slot_len(&self, slot_id: u8) -> usize {
        self.slot_lens
            .get(usize::from(slot_id))
            .copied()
            .unwrap_or(0)
    }

    /// Go: `getSlot`, the live pointers of slot `slot_id`.
    pub(super) fn slot(&self, slot_id: u8) -> &[EntryPtr] {
        let start = usize::from(slot_id) * self.slot_cap;
        self.slots_data
            .get(start..start + self.slot_len(slot_id))
            .unwrap_or_default()
    }

    /// Go: `getSlot`, for writing.
    fn slot_mut(&mut self, slot_id: u8) -> &mut [EntryPtr] {
        let start = usize::from(slot_id) * self.slot_cap;
        let len = self.slot_len(slot_id);
        self.slots_data
            .get_mut(start..start + len)
            .unwrap_or_default()
    }

    /// Go: `slot[idx].offset`.
    fn ptr_offset(&self, slot_id: u8, idx: usize) -> i64 {
        self.slot(slot_id).get(idx).map_or(0, |ptr| ptr.offset)
    }

    /// Go: `seg.rb.ReadAt(hdrBuf[:], offset)` into a zeroed header buffer.
    pub(super) fn read_hdr(&self, offset: i64) -> EntryHdr {
        let mut buf = [0; ENTRY_HDR_SIZE];
        let _ = self.rb.read_at(&mut buf, offset);
        EntryHdr::decode(buf)
    }

    /// Go: `seg.rb.WriteAt(hdrBuf[:], offset)`.
    fn write_hdr(&mut self, hdr: &EntryHdr, offset: i64) {
        let _ = self.rb.write_at(&hdr.encode(), offset);
    }

    /// Go: `seg.rb`.
    pub(super) const fn rb(&self) -> &RingBuf {
        &self.rb
    }

    /// Feeds the whole segment state, Go's unexported fields included, to a parity digest in the
    /// order `testdata/go` reads them with `reflect`.
    #[cfg(test)]
    pub(super) fn digest(&self, stats: &SegmentStats, digest: &mut Fnv) {
        digest.i64(self.rb.begin());
        digest.i64(self.rb.end());
        digest.i64(self.rb.write_index());
        digest.i64(wide(self.rb.data().len()));
        digest.words(self.rb.data());
        digest.i64(self.vacuum_len);
        digest.i64(wide(self.slot_cap));
        for counter in [
            &stats.miss_count,
            &stats.hit_count,
            &stats.entry_count,
            &stats.total_count,
            &stats.total_time,
            &stats.total_evacuate,
            &stats.total_expired,
            &stats.overwrites,
            &stats.touched,
        ] {
            digest.i64(load(counter));
        }
        for slot_id in 0..=u8::MAX {
            let slot = self.slot(slot_id);
            digest.i64(wide(slot.len()));
            for ptr in slot {
                digest.i64(ptr.offset);
                digest.i64(i64::from(ptr.hash16));
                digest.i64(i64::from(ptr.key_len));
            }
        }
    }
}

/// Go: `entryPtrIdx`, the first index in `slot` whose `hash16` is not below `hash16`.
fn entry_ptr_idx(slot: &[EntryPtr], hash16: u16) -> usize {
    let mut idx = 0;
    let mut high = slot.len();
    while idx < high {
        let mid = usize::midpoint(idx, high);
        if slot
            .get(mid)
            .is_some_and(|old_entry| old_entry.hash16 < hash16)
        {
            idx = mid + 1;
        } else {
            high = mid;
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::{
        super::golden::{TestResult, check_eq},
        CacheError, ENTRY_HDR_SIZE, EntryHdr,
    };

    #[test]
    fn errors_have_go_texts() -> TestResult {
        let cases = [
            (CacheError::LargeKey, "The key is larger than 65535"),
            (
                CacheError::LargeEntry,
                "The entry size is larger than 1/1024 of cache size",
            ),
            (CacheError::NotFound, "Entry not found"),
            (CacheError::OutOfRange, "out of range"),
            (CacheError::Eof, "EOF"),
        ];
        for (err, text) in cases {
            check_eq(&err.to_string(), &text.to_owned(), &format!("{err:?}"))?;
        }
        Ok(())
    }

    #[test]
    fn header_round_trips_in_go_layout() -> TestResult {
        let hdr = EntryHdr {
            access_time: 0x0403_0201,
            expire_at: 0x0807_0605,
            key_len: 0x0A09,
            hash16: 0x0C0B,
            val_len: 0x100F_0E0D,
            val_cap: 0x1413_1211,
            deleted: true,
            slot_id: 0x16,
            reserved: 0x1817,
        };
        let bytes = hdr.encode();
        let want: [u8; ENTRY_HDR_SIZE] = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 1, 22, 23, 24,
        ];
        check_eq(&bytes, &want, "encoded header")?;
        check_eq(&EntryHdr::decode(bytes), &hdr, "decoded header")
    }
}
