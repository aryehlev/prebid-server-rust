//! Bit-level writes used to encode GPP strings. Go: `util/encoding.go`.
//!
//! The Go methods live on `BitStream`; here they are free functions that
//! [`BitStream`]'s single inherent `impl` block delegates to.

use super::{
    base64::raw_url_encode,
    bitstream::{BitStream, shl8, shr8},
    fibonacci::{FIB_LEN, fib_at},
    rangesection::IntRange,
};
use crate::error::GppError;

/// Go: `fibEncodeNumOutOfRangeErr`.
const FIB_ENCODE_NUM_OUT_OF_RANGE: &str = "the number to be encoded is out of range";
/// Go: `fibEncodeInvalidRange`.
const FIB_ENCODE_INVALID_RANGE: &str = "the range is invalid";

/// Most strings to be encoded are less than 8 bytes. Go: `getByteSlice`.
pub(super) fn byte_slice() -> Vec<u8> {
    Vec::with_capacity(8)
}

/// Go: `BitStream.enlarge`. Grows the buffer so it holds `n` more bits past the position,
/// following Go's capacity policy (the policy itself is not observable).
fn enlarge(bs: &mut BitStream, n: u16) {
    let final_len = usize::from(bs.p.wrapping_add(n).wrapping_add(7)) / 8;
    if final_len <= bs.b.capacity() {
        bs.b.resize(final_len, 0);
        return;
    }

    let mut new_cap = bs.b.capacity();
    let double_cap = new_cap + new_cap;
    if final_len > double_cap {
        new_cap = final_len;
    } else if new_cap < 32 {
        new_cap = double_cap;
    } else {
        while new_cap < final_len {
            new_cap += new_cap / 4;
        }
    }
    bs.b.reserve_exact(new_cap.saturating_sub(bs.b.len()));
    bs.b.resize(final_len, 0);
}

/// Appends the first `n` bits of `b` (most significant first). Go: `BitStream.appendNBits`.
fn append_n_bits(bs: &mut BitStream, b: &[u8], n: u16) {
    enlarge(bs, n);
    for i in 0..n {
        let byte_index = usize::from(bs.p / 8);
        let offset = bs.p % 8;
        let source = b.get(usize::from(i / 8)).copied().unwrap_or_default();
        let bit = shr8(shl8(source, i % 8) & 0x80, offset);
        if let Some(target) = bs.b.get_mut(byte_index) {
            *target |= bit;
        }
        bs.p = bs.p.wrapping_add(1);
    }
}

/// Go: `BitStream.Base64Encode`.
pub(super) fn base64_encode(bs: &BitStream) -> Vec<u8> {
    raw_url_encode(&bs.b)
}

/// Go: `BitStream.WriteByte1`.
pub(super) fn write_byte1(bs: &mut BitStream, b: u8) {
    append_n_bits(bs, &[shl8(b, 7)], 1);
}

/// Go: `BitStream.WriteByte2`.
pub(super) fn write_byte2(bs: &mut BitStream, b: u8) {
    append_n_bits(bs, &[shl8(b, 6)], 2);
}

/// Go: `BitStream.WriteByte4`.
pub(super) fn write_byte4(bs: &mut BitStream, b: u8) {
    append_n_bits(bs, &[shl8(b, 4)], 4);
}

/// Go: `BitStream.WriteByte6`.
pub(super) fn write_byte6(bs: &mut BitStream, b: u8) {
    append_n_bits(bs, &[shl8(b, 2)], 6);
}

/// Go: `BitStream.WriteByte8`.
pub(super) fn write_byte8(bs: &mut BitStream, b: u8) {
    append_n_bits(bs, &[b], 8);
}

/// Go's `byte(x)`: the low 8 bits.
const fn low_byte(x: u16) -> u8 {
    let [_, low] = x.to_be_bytes();
    low
}

/// Go: `BitStream.WriteUInt12`.
pub(super) fn write_uint12(bs: &mut BitStream, b: u16) {
    let first = low_byte(b >> 4);
    let second = low_byte(b << 4);
    append_n_bits(bs, &[first, second], 12);
}

/// Go: `BitStream.WriteUInt16`.
pub(super) fn write_uint16(bs: &mut BitStream, b: u16) {
    append_n_bits(bs, &b.to_be_bytes(), 16);
}

/// Go: `BitStream.WriteTwoBitField`.
pub(super) fn write_two_bit_field(bs: &mut BitStream, list: &[u8]) {
    for &b in list {
        write_byte2(bs, b);
    }
}

/// Go: `BitStream.WriteFibonacciInt`.
pub(super) fn write_fibonacci_int(bs: &mut BitStream, num: u16) -> Result<(), GppError> {
    // The num should be [1,6765).
    if num == 0 || num >= fib_at(FIB_LEN - 1) + fib_at(FIB_LEN - 2) {
        return Err(GppError::new(FIB_ENCODE_NUM_OUT_OF_RANGE.to_owned()));
    }
    // Binary search for the largest fibonacci number less than or equal to num.
    let (mut lo, mut hi) = (2usize, FIB_LEN - 1);
    while lo < hi {
        let mid = usize::midpoint(lo, hi);
        if num >= fib_at(mid) && num < fib_at(mid + 1) {
            lo = mid;
            break;
        }
        if num < fib_at(mid) {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    // Calculate the fibonacci-encoded sequence.
    let mut remaining = num;
    let mut fib_encoded: u32 = 1;
    for (offset, i) in (1u32..).zip((2..=lo).rev()) {
        if remaining >= fib_at(i) {
            remaining -= fib_at(i);
            fib_encoded |= 1 << offset;
        }
    }
    let encoded_length = u16::try_from(lo).unwrap_or_default();
    fib_encoded <<= 32 - u32::from(encoded_length);

    append_n_bits(bs, &fib_encoded.to_be_bytes(), encoded_length);
    Ok(())
}

/// Go: `BitStream.WriteIntRange`.
pub(super) fn write_int_range(bs: &mut BitStream, int_range: &IntRange) -> Result<(), GppError> {
    write_uint12(bs, int_range.size);
    // Assume that the ranges are ordered.
    let mut prev_id = 0u16;
    for r in &int_range.range {
        if r.end_id < r.start_id || prev_id >= r.start_id {
            return Err(GppError::new(FIB_ENCODE_INVALID_RANGE.to_owned()));
        }
        if r.start_id == r.end_id {
            write_byte1(bs, 0);
            write_fibonacci_int(bs, r.start_id - prev_id)?;
        } else {
            write_byte1(bs, 1);
            write_fibonacci_int(bs, r.start_id - prev_id)?;
            write_fibonacci_int(bs, r.end_id - r.start_id)?;
        }
        prev_id = r.end_id;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Go: `util/encoding_test.go`.

    use super::{FIB_ENCODE_INVALID_RANGE, FIB_ENCODE_NUM_OUT_OF_RANGE, enlarge};
    use crate::{
        error::GppError,
        util::{BitStream, IRange, IntRange},
    };

    /// Go: `prepareNBits`. Reads `n` bits of `data` starting at bit `pointer`.
    fn prepare_n_bits(data: &[u8], pointer: usize, n: usize) -> u16 {
        let mut data_to_write = 0u16;
        for (offset, i) in (0u32..).zip((pointer..pointer + n).rev()) {
            let byte = data.get(i / 8).copied().unwrap_or_default();
            let bit = u16::from(byte >> (7 - i % 8)) & 0x0001;
            data_to_write |= bit << offset;
        }
        data_to_write
    }

    fn low(x: u16) -> u8 {
        super::low_byte(x)
    }

    /// Name, values to write, expected bytes, expected error.
    type FibCase<'a> = (&'a str, &'a [u16], &'a [u8], Option<&'a str>);
    /// Name, ranges, expected bytes, expected error.
    type RangeCase<'a> = (&'a str, Vec<IRange>, &'a [u8], Option<&'a str>);

    const STRING_DATA: &[u8] = b"too young too simple sometimes naive";
    const UNICODE_DATA: &[u8] = &[0x04, 0xA2, 0x03, 0xB1, 0x00, 0x2B];

    /// Writes `data` back in `width`-bit chunks and checks the buffer reproduces it.
    fn round_trip(cases: &[(&str, &[u8])], width: usize, write: fn(&mut BitStream, u16)) {
        for &(name, data) in cases {
            let mut bs = BitStream::new(Vec::new());
            let mut p = 0;
            while p < data.len() * 8 {
                write(&mut bs, prepare_n_bits(data, p, width));
                p += width;
            }
            assert_eq!(bs.bytes(), data, "{name}");
        }
    }

    #[test]
    fn enlarge_capacity() {
        let cases: &[(&str, usize, usize, u16, usize)] = &[
            ("test_no_exceed", 3, 8, 3, 8),
            ("test_no_exceed_boundary", 3, 8, 5, 8),
            ("test_exceed_doubling", 3, 8, 6, 16),
            ("test_exceed_quartering_1", 33, 33, 1, 33 + 33 / 4),
            ("test_exceed_quartering_2", 40, 40, 30, 77),
            ("test_exceed_direct_set", 0, 3, 9, 9),
            ("test_expand_zero", 0, 0, 0, 0),
        ];
        for &(name, len, cap, n, expect) in cases {
            let mut bytes = Vec::with_capacity(cap);
            bytes.resize(len, 0);
            let mut bs = BitStream::new(bytes);
            bs.set_position(u16::try_from(len * 8).unwrap_or_default());
            enlarge(&mut bs, n * 8);
            // Rust only guarantees a lower bound on Vec capacity.
            assert!(bs.b.capacity() >= expect, "{name}: capacity");
            assert_eq!(bs.b.len(), len + usize::from(n), "{name}: length");
        }
    }

    #[test]
    fn write_byte1() {
        let cases: &[(&str, &[u8])] = &[
            ("single_byte", b"a"),
            ("bytes_string", STRING_DATA),
            ("bytes_unicode", UNICODE_DATA),
        ];
        round_trip(cases, 1, |bs, v| bs.write_byte1(low(v)));
    }

    #[test]
    fn write_byte2() {
        let cases: &[(&str, &[u8])] = &[
            ("single_byte", b"a"),
            ("bytes_string", STRING_DATA),
            ("bytes_unicode", UNICODE_DATA),
        ];
        round_trip(cases, 2, |bs, v| bs.write_byte2(low(v)));
    }

    #[test]
    fn write_byte4() {
        let cases: &[(&str, &[u8])] = &[
            ("nil_bytes", &[]),
            ("single_byte", b"a"),
            ("bytes_string", STRING_DATA),
            ("bytes_unicode", UNICODE_DATA),
        ];
        round_trip(cases, 4, |bs, v| bs.write_byte4(low(v)));
    }

    #[test]
    fn write_byte6() {
        let cases: &[(&str, &[u8])] = &[
            ("nil_bytes", &[]),
            ("triple_bytes", b"aaa"),
            ("bytes_string", STRING_DATA),
            ("bytes_unicode", UNICODE_DATA),
        ];
        round_trip(cases, 6, |bs, v| bs.write_byte6(low(v)));
    }

    #[test]
    fn write_byte8() {
        let cases: &[(&str, &[u8])] = &[
            ("nil_bytes", &[]),
            ("single_byte", b"a"),
            ("bytes_string", STRING_DATA),
            ("bytes_unicode", UNICODE_DATA),
        ];
        round_trip(cases, 8, |bs, v| bs.write_byte8(low(v)));
    }

    #[test]
    fn write_uint12() {
        let cases: &[(&str, &[u8])] = &[
            ("triple_bytes", b"aaa"),
            ("bytes_string", STRING_DATA),
            ("bytes_unicode", UNICODE_DATA),
        ];
        round_trip(cases, 12, BitStream::write_uint12);
    }

    #[test]
    fn write_uint16() {
        let cases: &[(&str, &[u8])] = &[
            ("double_bytes", b"aa"),
            ("bytes_string", STRING_DATA),
            ("bytes_unicode", UNICODE_DATA),
        ];
        round_trip(cases, 16, BitStream::write_uint16);
    }

    #[test]
    fn arbitrary_write() {
        let cases: &[(&str, &[u8], &[usize])] = &[
            ("arbitrary_write_1", &[0xDE, 0xAD, 0xBE, 0xEF], &[16, 12, 4]),
            ("arbitrary_write_2", &[0xDE, 0xAD, 0xBE, 0xEF], &[12, 12, 8]),
            (
                "arbitrary_write_3",
                &[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0b0100_0000],
                &[16, 12, 4, 8, 6, 6, 6, 1, 4, 1, 2],
            ),
            (
                "arbitrary_write_4",
                &[0x81, 0x09, 0x75],
                &[1, 4, 2, 4, 6, 2, 4, 1],
            ),
            (
                "arbitrary_write_string",
                b"GPP framework",
                &[16, 16, 8, 8, 8, 4, 4, 4, 4, 6, 6, 6, 6, 4, 2, 1, 1],
            ),
        ];
        for &(name, data, offsets) in cases {
            let mut bs = BitStream::new(Vec::new());
            let mut p = 0;
            for &offset in offsets {
                let v = prepare_n_bits(data, p, offset);
                match offset {
                    1 => bs.write_byte1(low(v)),
                    2 => bs.write_byte2(low(v)),
                    4 => bs.write_byte4(low(v)),
                    6 => bs.write_byte6(low(v)),
                    8 => bs.write_byte8(low(v)),
                    12 => bs.write_uint12(v),
                    16 => bs.write_uint16(v),
                    _ => {}
                }
                p += offset;
            }
            assert_eq!(bs.bytes(), data, "{name}");
        }
    }

    #[test]
    fn write_fibonacci_int() {
        let cases: &[FibCase<'_>] = &[
            ("write_int_1", &[1], &[0xC0], None),
            ("write_int_2", &[2], &[0x60], None),
            ("write_int_4", &[4], &[0xB0], None),
            ("write_int_5", &[5], &[0x18], None),
            ("write_int_7_12", &[7, 12], &[0x5D, 0x60], None),
            (
                "out_of_range",
                &[0, 6765],
                &[],
                Some(FIB_ENCODE_NUM_OUT_OF_RANGE),
            ),
            ("write_int_6764", &[6764], &[0x55, 0x55, 0x60], None),
        ];
        for &(name, values, result, err) in cases {
            let mut bs = BitStream::new(Vec::new());
            for &v in values {
                let got = bs.write_fibonacci_int(v).err();
                assert_eq!(got.as_ref().map(GppError::message), err, "{name}: error");
            }
            assert_eq!(bs.bytes(), result, "{name}: bytes");
        }
    }

    #[test]
    fn write_int_range() {
        let range = |pairs: &[(u16, u16)]| -> Vec<IRange> {
            pairs
                .iter()
                .map(|&(start_id, end_id)| IRange { start_id, end_id })
                .collect()
        };
        let cases: Vec<RangeCase<'_>> = vec![
            (
                "write_int_range_1",
                range(&[(2, 2), (4, 4), (6, 9)]),
                &[0b0000_0000, 0b0011_0011, 0b0011_1011, 0b0011_0000],
                None,
            ),
            (
                "write_int_range_2",
                range(&[(2, 5), (6, 8), (13, 16), (28, 39)]),
                &[
                    0b0000_0000,
                    0b0100_1011,
                    0b0011_1110,
                    0b1110_0011,
                    0b0011_1101,
                    0b0110_0101,
                    0b1000_0000,
                ],
                None,
            ),
            (
                "write_invalid_range_1",
                range(&[(2, 1), (4, 4), (6, 9)]),
                &[0b0000_0000, 0b0011_0000],
                Some(FIB_ENCODE_INVALID_RANGE),
            ),
            (
                "write_invalid_range_2",
                range(&[(2, 2), (2, 4), (6, 9)]),
                &[0b0000_0000, 0b0011_0011],
                Some(FIB_ENCODE_INVALID_RANGE),
            ),
            (
                "write_out_of_range",
                range(&[(2, 5), (6, 8), (13, 16), (28, 9999)]),
                &[
                    0b0000_0000,
                    0b0100_1011,
                    0b0011_1110,
                    0b1110_0011,
                    0b0011_1101,
                    0b0110_0000,
                ],
                Some(FIB_ENCODE_NUM_OUT_OF_RANGE),
            ),
        ];
        for (name, r, result, err) in cases {
            let mut bs = BitStream::new(Vec::new());
            let size = u16::try_from(r.len()).unwrap_or_default();
            let int_range = IntRange {
                size,
                range: r,
                max: 0,
            };
            let got = bs.write_int_range(&int_range).err();
            assert_eq!(bs.bytes(), result, "{name}: bytes");
            assert_eq!(got.as_ref().map(GppError::message), err, "{name}: error");
        }
    }
}
