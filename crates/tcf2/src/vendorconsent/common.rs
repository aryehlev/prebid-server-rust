//! Helpers shared by the TCF 1 and TCF 2 parsers.
//!
//! go-gdpr duplicates these in `vendorconsent/tcf1/metadata.go` and
//! `vendorconsent/tcf2/metadata.go`; the bit layout of the fields below is identical in both
//! formats.

use crate::{ConsentError, api::ConsentTime};

/// Returns `data[index]`, or 0 past the end.
///
/// Callers only read bytes that the parsers have already bounds-checked with Go's own
/// conditions, so the fallback is never used; it keeps the port free of panicking indexing.
pub(super) fn byte(data: &[u8], index: usize) -> u8 {
    data.get(index).copied().unwrap_or_default()
}

/// Port of the unexported `isSet` helpers: whether bit `bit_index` of `data` is 1.
///
/// Go indexes `data[bit_index/8]` directly and panics past the end of the slice; that panic is
/// returned as [`ConsentError::IndexOutOfRange`] with Go's index and length.
pub(super) fn is_set(data: &[u8], bit_index: usize) -> Result<bool, ConsentError> {
    let index = bit_index / 8;
    let value = data
        .get(index)
        .copied()
        .ok_or(ConsentError::IndexOutOfRange {
            index,
            length: data.len(),
        })?;
    Ok(value & (0x80 >> (bit_index % 8)) != 0)
}

/// [`is_set`] for bits the parsers have already bounds-checked.
pub(super) fn bit(data: &[u8], bit_index: usize) -> bool {
    matches!(is_set(data, bit_index), Ok(true))
}

/// Go: `Version()`, bits 0-5.
pub(super) fn version(data: &[u8]) -> u8 {
    byte(data, 0) >> 2
}

const fn deciseconds(bytes: [u8; 5]) -> ConsentTime {
    let [b0, b1, b2, b3, b4] = bytes;
    ConsentTime::from_deciseconds(u64::from_be_bytes([0, 0, 0, b0, b1, b2, b3, b4]))
}

/// Go: `Created()`, bits 6-41.
pub(super) fn created(data: &[u8]) -> ConsentTime {
    let c = |i| byte(data, i);
    deciseconds([
        (c(0) & 0x3) << 2 | c(1) >> 6,
        c(1) << 2 | c(2) >> 6,
        c(2) << 2 | c(3) >> 6,
        c(3) << 2 | c(4) >> 6,
        c(4) << 2 | c(5) >> 6,
    ])
}

/// Go: `LastUpdated()`, bits 42-77.
pub(super) fn last_updated(data: &[u8]) -> ConsentTime {
    let c = |i| byte(data, i);
    deciseconds([
        (c(5) >> 2) & 0x0F,
        c(5) << 6 | c(6) >> 2,
        c(6) << 6 | c(7) >> 2,
        c(7) << 6 | c(8) >> 2,
        c(8) << 6 | c(9) >> 2,
    ])
}

/// Go: `CmpID()`, bits 78-89.
pub(super) fn cmp_id(data: &[u8]) -> u16 {
    let c = |i| byte(data, i);
    u16::from_be_bytes([((c(9) & 0x03) << 2) | c(10) >> 6, (c(10) << 2) | c(11) >> 6])
}

/// Go: `CmpVersion()`, bits 90-101.
pub(super) fn cmp_version(data: &[u8]) -> u16 {
    let c = |i| byte(data, i);
    u16::from_be_bytes([(c(11) >> 2) & 0x0F, (c(11) << 6) | c(12) >> 2])
}

/// Go: `ConsentScreen()`, bits 102-107.
pub(super) fn consent_screen(data: &[u8]) -> u8 {
    ((byte(data, 12) & 0x03) << 4) | byte(data, 13) >> 4
}

/// Go: `ConsentLanguage()`, bits 108-119: two 6-bit letters, each plus 65.
pub(super) fn consent_language(data: &[u8]) -> [u8; 2] {
    let left_char = ((byte(data, 13) & 0x0F) << 2) | byte(data, 14) >> 6;
    let right_char = byte(data, 14) & 0x3F;
    // Both values are below 64, so adding 65 cannot overflow.
    [left_char + 65, right_char + 65]
}

/// Go: `VendorListVersion()`, bits 120-131.
pub(super) fn vendor_list_version(data: &[u8]) -> u16 {
    let right_byte = ((byte(data, 16) & 0xF0) >> 4) | ((byte(data, 15) & 0x0F) << 4);
    let left_byte = byte(data, 15) >> 4;
    u16::from_be_bytes([left_byte, right_byte])
}

/// A `RangeSection` entry: the inclusive vendor ID range `[start_id, end_id]`.
///
/// Go: `rangeConsent` (`tcf2/rangesection.go`) and `rangeException` (`tcf1/rangesection.go`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RangeEntry {
    pub(super) start_id: u16,
    pub(super) end_id: u16,
}

impl RangeEntry {
    /// Go: `Contains`.
    pub(super) const fn contains(self, id: u16) -> bool {
        self.start_id <= id && self.end_id >= id
    }
}
