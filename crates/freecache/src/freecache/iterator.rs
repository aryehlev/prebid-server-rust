//! Port of freecache `iterator.go`.

use std::iter::FusedIterator;

use super::{
    cache::{Cache, SEGMENT_COUNT, Segment},
    segment::{HDR_SIZE, SegmentState},
    timer::Timer,
};

/// The number of slots per segment, as `u8` slot ids span.
const SLOT_COUNT: usize = 256;

/// Go: `Entry`, a key/value pair returned by the iterator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Go: `Key`.
    pub key: Vec<u8>,
    /// Go: `Value`.
    pub value: Vec<u8>,
}

/// Go: `Iterator`, which walks the cache segment by segment and slot by slot.
///
/// Each [`Iterator::next`] call locks one segment at a time, so entries added or removed
/// concurrently may or may not be seen. Expired entries are skipped (not removed). The order
/// is not guaranteed by Go, but it is the same as Go's for the same operations.
#[derive(Debug)]
pub struct CacheIterator<'cache, T> {
    cache: &'cache Cache<T>,
    segment_idx: usize,
    slot_idx: usize,
    entry_idx: usize,
}

impl<'cache, T> CacheIterator<'cache, T> {
    /// Go: `NewIterator`.
    pub(super) const fn new(cache: &'cache Cache<T>) -> Self {
        Self {
            cache,
            segment_idx: 0,
            slot_idx: 0,
            entry_idx: 0,
        }
    }

    /// Go: `nextForSegment`, holding the segment lock while it scans.
    fn next_for_segment(&mut self, seg: &Segment) -> Option<Entry>
    where
        T: Timer,
    {
        let state = seg.lock();
        let entry = self.scan_segment(&state);
        drop(state);
        entry
    }

    /// Go: the slot loop of `nextForSegment`.
    fn scan_segment(&mut self, state: &SegmentState) -> Option<Entry>
    where
        T: Timer,
    {
        while self.slot_idx < SLOT_COUNT {
            let Ok(slot_id) = u8::try_from(self.slot_idx) else {
                break;
            };
            if let Some(entry) = self.next_for_slot(state, slot_id) {
                return Some(entry);
            }
            self.slot_idx += 1;
            self.entry_idx = 0;
        }
        None
    }

    /// Go: `nextForSlot`.
    fn next_for_slot(&mut self, state: &SegmentState, slot_id: u8) -> Option<Entry>
    where
        T: Timer,
    {
        let slot = state.slot(slot_id);
        while let Some(ptr) = slot.get(self.entry_idx) {
            self.entry_idx += 1;
            let now = self.cache.timer().now();
            let hdr = state.read_hdr(ptr.offset);
            if hdr.expire_at == 0 || hdr.expire_at > now {
                let mut entry = Entry {
                    key: vec![0; usize::from(hdr.key_len)],
                    value: vec![0; usize::try_from(hdr.val_len).unwrap_or(0)],
                };
                let _ = state.rb().read_at(&mut entry.key, ptr.offset + HDR_SIZE);
                let _ = state.rb().read_at(
                    &mut entry.value,
                    ptr.offset + HDR_SIZE + i64::from(hdr.key_len),
                );
                return Some(entry);
            }
        }
        None
    }
}

impl<T: Timer> Iterator for CacheIterator<'_, T> {
    type Item = Entry;

    /// Go: `Next`, the next entry, or `None` when every segment has been walked.
    fn next(&mut self) -> Option<Entry> {
        while self.segment_idx < SEGMENT_COUNT {
            let cache = self.cache;
            if let Some(seg) = cache.segments().get(self.segment_idx)
                && let Some(entry) = self.next_for_segment(seg)
            {
                return Some(entry);
            }
            self.segment_idx += 1;
            self.slot_idx = 0;
            self.entry_idx = 0;
        }
        None
    }
}

impl<T: Timer> FusedIterator for CacheIterator<'_, T> {}
