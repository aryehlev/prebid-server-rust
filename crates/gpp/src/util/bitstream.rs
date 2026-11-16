//! Bit-level reads from a byte buffer. Go: `util/bitstream.go`.
//!
//! Go's arithmetic is reproduced exactly, including its awkward corners:
//! - the bit position is a `uint16` that wraps at 65536;
//! - bounds checks compare against `uint16(len(data))` (the length truncated to 16 bits) while the
//!   messages print the real length;
//! - byte shifts by 8 or more yield 0 (Go semantics) instead of panicking;
//! - `ParseByte6` shifts by `2 - bitStartIndex` (not the in-byte offset) when the offset is below
//!   3, so reads at bit 1 or 2 are unmasked and reads at bit 8, 16, ... return 0.
//!
//! Go spreads the `BitStream` methods over `bitstream.go`, `encoding.go`, `fibonacci.go` and
//! `rangesection.go`; the Rust type has a single `impl` block here whose write, Fibonacci and
//! range methods delegate to the sibling modules named after those files.

use super::{base64, encoding, fibonacci, rangesection, rangesection::IntRange};
use crate::error::GppError;

/// A byte buffer with a bit cursor. Go: `util.BitStream`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitStream {
    /// Bit position. Go: `p uint16`.
    pub(crate) p: u16,
    /// Data. Go: `b []byte`.
    pub(crate) b: Vec<u8>,
}

impl BitStream {
    /// Creates a bit stream positioned at bit 0. Go: `util.NewBitStream`.
    #[must_use]
    pub const fn new(b: Vec<u8>) -> Self {
        Self { p: 0, b }
    }

    /// Creates an empty bit stream for writing. Go: `util.NewBitStreamForWrite`.
    #[must_use]
    pub fn new_for_write() -> Self {
        Self::new(encoding::byte_slice())
    }

    /// Clears all the data the stream holds. Go: `Reset`.
    pub fn reset(&mut self) {
        self.b = encoding::byte_slice();
        self.p = 0;
    }

    /// Base64url-encodes (unpadded) the bytes written so far. Go: `Base64Encode`.
    #[must_use]
    pub fn base64_encode(&self) -> Vec<u8> {
        encoding::base64_encode(self)
    }

    /// Writes the rightmost bit of `b`. Go: `WriteByte1`.
    pub fn write_byte1(&mut self, b: u8) {
        encoding::write_byte1(self, b);
    }

    /// Writes the rightmost 2 bits of `b`. Go: `WriteByte2`.
    pub fn write_byte2(&mut self, b: u8) {
        encoding::write_byte2(self, b);
    }

    /// Writes the rightmost 4 bits of `b`. Go: `WriteByte4`.
    pub fn write_byte4(&mut self, b: u8) {
        encoding::write_byte4(self, b);
    }

    /// Writes the rightmost 6 bits of `b`. Go: `WriteByte6`.
    pub fn write_byte6(&mut self, b: u8) {
        encoding::write_byte6(self, b);
    }

    /// Writes `b`. Go: `WriteByte8`.
    pub fn write_byte8(&mut self, b: u8) {
        encoding::write_byte8(self, b);
    }

    /// Writes the rightmost 12 bits of `b`. Go: `WriteUInt12`.
    pub fn write_uint12(&mut self, b: u16) {
        encoding::write_uint12(self, b);
    }

    /// Writes `b`. Go: `WriteUInt16`.
    pub fn write_uint16(&mut self, b: u16) {
        encoding::write_uint16(self, b);
    }

    /// Writes each value as 2 bits. Go: `WriteTwoBitField`.
    pub fn write_two_bit_field(&mut self, list: &[u8]) {
        encoding::write_two_bit_field(self, list);
    }

    /// Writes a Fibonacci-coded integer. Go: `WriteFibonacciInt`.
    ///
    /// # Errors
    ///
    /// Returns `the number to be encoded is out of range` unless `1 <= num < 6765`.
    pub fn write_fibonacci_int(&mut self, num: u16) -> Result<(), GppError> {
        encoding::write_fibonacci_int(self, num)
    }

    /// Writes a Fibonacci-coded range list. Go: `WriteIntRange`.
    ///
    /// # Errors
    ///
    /// Returns `the range is invalid` for unordered or inverted ranges, or the Fibonacci
    /// out-of-range error. Bits written before the error stay in the stream, as in Go.
    pub fn write_int_range(&mut self, int_range: &IntRange) -> Result<(), GppError> {
        encoding::write_int_range(self, int_range)
    }

    /// Reads a Fibonacci-coded integer. Go: `ReadFibonacciInt`.
    ///
    /// # Errors
    ///
    /// Returns Go's wrapped bit read error. Where Go would loop forever (a stream of at least
    /// 8192 bytes without any `11` bit pair) this returns a Rust-only error instead.
    pub fn read_fibonacci_int(&mut self) -> Result<u16, GppError> {
        fibonacci::read_fibonacci_int(self)
    }

    /// Reads a Fibonacci-coded range list. Go: `ReadFibonacciRange`.
    ///
    /// # Errors
    ///
    /// Returns Go's wrapped size, flag or offset read errors.
    pub fn read_fibonacci_range(&mut self) -> Result<IntRange, GppError> {
        fibonacci::read_fibonacci_range(self)
    }

    /// Reads a 16-bit-integer range list. Go: `ReadIntRange`.
    ///
    /// # Errors
    ///
    /// Returns Go's wrapped size, flag or value read errors.
    pub fn read_int_range(&mut self) -> Result<IntRange, GppError> {
        rangesection::read_int_range(self)
    }

    /// Creates a bit stream from a base64url string, padding an incomplete last quantum with an
    /// `A` first. Go: `util.NewBitStreamFromBase64`.
    ///
    /// # Errors
    ///
    /// Returns Go's `illegal base64 data at input byte N` error when the input is not valid
    /// unpadded base64url.
    pub fn from_base64(encoded: &str) -> Result<Self, GppError> {
        let decoded = if encoded.len().is_multiple_of(4) {
            base64::raw_url_decode(encoded.as_bytes())
        } else {
            // Go: padLastQuantum pads an incomplete last quantum with zeros.
            let mut padded = Vec::with_capacity(encoded.len() + 1);
            padded.extend_from_slice(encoded.as_bytes());
            padded.push(b'A');
            base64::raw_url_decode(&padded)
        };
        decoded
            .map(Self::new)
            .map_err(|e| GppError::new(e.to_string()))
    }

    /// Returns the bit position. Go: `GetPosition`.
    #[must_use]
    pub const fn position(&self) -> u16 {
        self.p
    }

    /// Sets the bit position. Go: `SetPosition`.
    pub const fn set_position(&mut self, pos: u16) {
        self.p = pos;
    }

    /// Returns the number of bytes, truncated to 16 bits like Go's `uint16(len(bs.b))`.
    /// Go: `Len`.
    #[must_use]
    pub fn len(&self) -> u16 {
        len16(&self.b)
    }

    /// Reports whether the stream holds no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.b.is_empty()
    }

    /// Returns the underlying bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.b
    }

    /// Advances the position by `n` bits with Go's `uint16` wrap-around.
    const fn advance(&mut self, n: u16) {
        self.p = self.p.wrapping_add(n);
    }

    /// Reads 1 bit. Go: `ReadByte1`.
    ///
    /// # Errors
    ///
    /// Returns Go's bounds error when the bit is past the end of the data.
    pub fn read_byte1(&mut self) -> Result<u8, GppError> {
        let b = parse_byte1(&self.b, self.p)?;
        self.advance(1);
        Ok(b)
    }

    /// Reads 2 bits. Go: `ReadByte2`.
    ///
    /// # Errors
    ///
    /// Returns Go's bounds error when the bits are past the end of the data.
    pub fn read_byte2(&mut self) -> Result<u8, GppError> {
        let b = parse_byte2(&self.b, self.p)?;
        self.advance(2);
        Ok(b)
    }

    /// Reads 4 bits. Go: `ReadByte4`.
    ///
    /// # Errors
    ///
    /// Returns Go's bounds error when the bits are past the end of the data.
    pub fn read_byte4(&mut self) -> Result<u8, GppError> {
        let b = parse_byte4(&self.b, self.p)?;
        self.advance(4);
        Ok(b)
    }

    /// Reads 6 bits. Go: `ReadByte6`.
    ///
    /// # Errors
    ///
    /// Returns Go's bounds error when the bits are past the end of the data.
    pub fn read_byte6(&mut self) -> Result<u8, GppError> {
        let b = parse_byte6(&self.b, self.p)?;
        self.advance(6);
        Ok(b)
    }

    /// Reads 8 bits. Go: `ReadByte8`.
    ///
    /// # Errors
    ///
    /// Returns Go's bounds error when the bits are past the end of the data.
    pub fn read_byte8(&mut self) -> Result<u8, GppError> {
        let b = parse_byte8(&self.b, self.p)?;
        self.advance(8);
        Ok(b)
    }

    /// Reads a 12-bit integer. Go: `ReadUInt12`.
    ///
    /// # Errors
    ///
    /// Returns Go's bounds error when the bits are past the end of the data.
    pub fn read_uint12(&mut self) -> Result<u16, GppError> {
        let i = parse_uint12(&self.b, self.p)?;
        self.advance(12);
        Ok(i)
    }

    /// Reads a 16-bit integer. Go: `ReadUInt16`.
    ///
    /// # Errors
    ///
    /// Returns Go's bounds error when the bits are past the end of the data.
    pub fn read_uint16(&mut self) -> Result<u16, GppError> {
        let i = parse_uint16(&self.b, self.p)?;
        self.advance(16);
        Ok(i)
    }

    /// Reads `num_fields` consecutive 2-bit values. Go: `ReadTwoBitField`.
    ///
    /// # Errors
    ///
    /// Returns `numFields is invalid` when `num_fields <= 0`, or the first 2-bit read error.
    pub fn read_two_bit_field(&mut self, num_fields: i64) -> Result<Vec<u8>, GppError> {
        if num_fields <= 0 {
            return Err(GppError::new("numFields is invalid".to_owned()));
        }
        let count = usize::try_from(num_fields).unwrap_or_default();
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            result.push(self.read_byte2()?);
        }
        Ok(result)
    }
}

/// Go's `uint16(len(data))`.
pub(super) fn len16(data: &[u8]) -> u16 {
    u16::try_from(data.len() % 65_536).unwrap_or_default()
}

/// `data[index]`. Every caller has already performed Go's bounds check, which is stricter than the
/// real length, so the fallback is never taken.
fn at(data: &[u8], index: u16) -> u8 {
    data.get(usize::from(index)).copied().unwrap_or_default()
}

/// Go's `x << s` on a byte: 0 when `s >= 8`, high bits discarded otherwise.
pub(super) fn shl8(x: u8, s: u16) -> u8 {
    x.checked_shl(u32::from(s)).unwrap_or(0)
}

/// Go's `x >> s` on a byte: 0 when `s >= 8`.
pub(super) fn shr8(x: u8, s: u16) -> u8 {
    x.checked_shr(u32::from(s)).unwrap_or(0)
}

/// Parses 1 bit starting at `bit_start_index`. Go: `util.ParseByte1`.
///
/// # Errors
///
/// Returns Go's bounds error when the bit is past the end of the data.
pub fn parse_byte1(data: &[u8], bit_start_index: u16) -> Result<u8, GppError> {
    let start_byte = bit_start_index / 8;
    let bit_offset = bit_start_index % 8;
    if len16(data) < start_byte + 1 {
        return Err(GppError::new(format!(
            "expected 1 bit at bit {bit_start_index}, but the byte array was only {} bytes long",
            data.len()
        )));
    }
    Ok(shr8(
        at(data, start_byte) & shr8(0x80, bit_offset),
        7 - bit_offset,
    ))
}

/// Parses 2 bits starting at `bit_start_index`. Go: `util.ParseByte2`.
///
/// # Errors
///
/// Returns Go's bounds error when the bits are past the end of the data.
pub fn parse_byte2(data: &[u8], bit_start_index: u16) -> Result<u8, GppError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    let error = || {
        GppError::new(format!(
            "expected 2 bits to start at bit {bit_start_index}, but the byte array was only {} \
             bytes long",
            data.len()
        ))
    };
    if bit_start_offset < 7 {
        if len16(data) < start_byte + 1 {
            return Err(error());
        }
        return Ok(shr8(
            at(data, start_byte) & shr8(0xC0, bit_start_offset),
            6 - bit_start_offset,
        ));
    }
    if len16(data) < start_byte + 2 && bit_start_offset > 6 {
        return Err(error());
    }

    let left_bits = shl8(at(data, start_byte) & shr8(0xC0, bit_start_offset), 1);
    let overflow = 1;
    let right_bits = shr8(at(data, start_byte + 1) & shl8(0xC0, overflow), 7);
    Ok(left_bits | right_bits)
}

/// Parses 4 bits starting at `bit_start_index`. Go: `util.ParseByte4`.
///
/// # Errors
///
/// Returns Go's bounds error when the bits are past the end of the data.
pub fn parse_byte4(data: &[u8], bit_start_index: u16) -> Result<u8, GppError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    if bit_start_offset < 5 {
        if len16(data) < start_byte + 1 {
            return Err(GppError::new(format!(
                "expected 4 bits to start at bit {bit_start_index}, but the byte array was only \
                 {} bytes long",
                data.len()
            )));
        }
        return Ok(shr8(
            at(data, start_byte) & shr8(0xF0, bit_start_offset),
            4 - bit_start_offset,
        ));
    }
    if len16(data) < start_byte + 2 && bit_start_offset > 4 {
        return Err(GppError::new(format!(
            "expected 4 bits to start at bit {bit_start_index}, but the byte array was only {} \
             bytes long (needs second byte)",
            data.len()
        )));
    }

    let left_bits = shl8(
        at(data, start_byte) & shr8(0xF0, bit_start_offset),
        bit_start_offset - 4,
    );
    let bits_consumed = 8 - bit_start_offset;
    let overflow = 4 - bits_consumed;
    let right_bits = shr8(
        at(data, start_byte + 1) & shl8(0xF0, 4 - overflow),
        8 - overflow,
    );
    Ok(left_bits | right_bits)
}

/// Parses 6 bits starting at `bit_start_index`. Go: `util.ParseByte6`.
///
/// # Errors
///
/// Returns Go's bounds error when the bits are past the end of the data.
pub fn parse_byte6(data: &[u8], bit_start_index: u16) -> Result<u8, GppError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    if bit_start_offset < 3 {
        if len16(data) < start_byte + 1 {
            return Err(GppError::new(format!(
                "expected 6 bits to start at bit {bit_start_index}, but the byte array was only \
                 {} bytes long",
                data.len()
            )));
        }
        // Go shifts by `2 - bitStartIndex` (uint16, so it wraps), not by the in-byte offset.
        return Ok(shr8(
            at(data, start_byte),
            2u16.wrapping_sub(bit_start_index),
        ));
    }
    if len16(data) < start_byte + 2 {
        return Err(GppError::new(format!(
            "expected 6 bits to start at bit {bit_start_index}, but the byte array was only {} \
             bytes long (needs second byte)",
            data.len()
        )));
    }

    let left_bits = shl8(
        at(data, start_byte) & shr8(0xFC, bit_start_offset),
        bit_start_offset - 2,
    );
    let bits_consumed = 8 - bit_start_offset;
    let overflow = 6 - bits_consumed;
    let right_bits = shr8(
        at(data, start_byte + 1) & shl8(0xFC, 6 - overflow),
        8 - overflow,
    );
    Ok(left_bits | right_bits)
}

/// Parses 8 bits starting at `bit_start_index`. Go: `util.ParseByte8`.
///
/// # Errors
///
/// Returns Go's bounds error when the bits are past the end of the data.
pub fn parse_byte8(data: &[u8], bit_start_index: u16) -> Result<u8, GppError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    let error = || {
        GppError::new(format!(
            "expected 8 bits to start at bit {bit_start_index}, but the byte array was only {} \
             bytes long",
            data.len()
        ))
    };
    if bit_start_offset == 0 {
        if len16(data) < start_byte + 1 {
            return Err(error());
        }
        return Ok(at(data, start_byte));
    }
    if len16(data) < start_byte + 2 {
        return Err(error());
    }

    let left_bits = shl8(
        at(data, start_byte) & shr8(0xFF, bit_start_offset),
        bit_start_offset,
    );
    let shift_complement = 8 - bit_start_offset;
    let right_bits = shr8(
        at(data, start_byte + 1) & shl8(0xFF, shift_complement),
        shift_complement,
    );
    Ok(left_bits | right_bits)
}

/// Parses a 12-bit integer starting at `bit_start_index`. Go: `util.ParseUInt12`.
///
/// # Errors
///
/// Returns Go's bounds error when the bits are past the end of the data.
pub fn parse_uint12(data: &[u8], bit_start_index: u16) -> Result<u16, GppError> {
    let end = bit_start_index.wrapping_add(12);
    let mut end_byte = end / 8;
    let end_offset = end % 8;

    if end_offset > 0 {
        end_byte += 1;
    }
    if len16(data) < end_byte {
        return Err(GppError::new(format!(
            "expected a 12-bit int to start at bit {bit_start_index}, but the byte array was only \
             {} bytes long",
            data.len()
        )));
    }

    let left_byte = parse_byte4(data, bit_start_index).map_err(|err| {
        GppError::new(format!(
            "error reading first 4 bits of a 12 bit integer: {err}"
        ))
    })?;
    let right_byte = parse_byte8(data, bit_start_index.wrapping_add(4)).map_err(|err| {
        GppError::new(format!(
            "error reading the last 8 bits of a 12 bit integer: {err}"
        ))
    })?;
    Ok(u16::from_be_bytes([left_byte, right_byte]))
}

/// Parses a 16-bit integer starting at `bit_start_index`. Go: `util.ParseUInt16`.
///
/// # Errors
///
/// Returns Go's bounds error when the bits are past the end of the data.
pub fn parse_uint16(data: &[u8], bit_start_index: u16) -> Result<u16, GppError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    let error = || {
        GppError::new(format!(
            "expected a 16-bit int to start at bit {bit_start_index}, but the byte array was only \
             {} bytes long",
            data.len()
        ))
    };
    if bit_start_offset == 0 {
        if len16(data) < start_byte + 2 {
            return Err(error());
        }
        return Ok(u16::from_be_bytes([
            at(data, start_byte),
            at(data, start_byte + 1),
        ]));
    }
    if len16(data) < start_byte + 3 {
        return Err(error());
    }

    let left_byte = parse_byte8(data, bit_start_index).map_err(|err| {
        GppError::new(format!(
            "error reading the first 8 bits of a 16 bit integer: {err}"
        ))
    })?;
    let right_byte = parse_byte8(data, bit_start_index.wrapping_add(8)).map_err(|err| {
        GppError::new(format!(
            "error reading the last 8 bits of a 16 bit integer: {err}"
        ))
    })?;
    Ok(u16::from_be_bytes([left_byte, right_byte]))
}

#[cfg(test)]
mod tests {
    //! Go: `util/bitstream_test.go`.

    use super::BitStream;
    use crate::error::GppError;

    // 0000 0100 1010 0010 0000 0011 1011 0001 0000 0000 0010 1011
    const TEST_DATA: [u8; 6] = [0x04, 0xA2, 0x03, 0xB1, 0x00, 0x2B];

    /// One Go `testDefinition` row: name, data, offset, value, error.
    type Row<'a> = (&'a str, &'a [u8], u16, u64, &'a str);
    /// One Go `bitFieldTestDefinition` row: name, data, offset, fields, value, error.
    type TwoBitRow<'a> = (&'a str, &'a [u8], u16, i64, &'a [u8], &'a str);

    fn stream(data: &[u8], offset: u16) -> BitStream {
        BitStream {
            p: offset,
            b: data.to_vec(),
        }
    }

    /// Runs a Go table where reads of `width` bits must return `value` and advance the position
    /// (`check_position` mirrors whether the Go test asserts the position).
    fn run_rows<T>(
        rows: &[Row<'_>],
        width: u16,
        check_position: bool,
        read: impl Fn(&mut BitStream) -> Result<T, GppError>,
    ) where
        T: Into<u64> + Copy,
    {
        for &(name, data, offset, value, err) in rows {
            let mut bs = stream(data, offset);
            match read(&mut bs) {
                Ok(got) => {
                    assert!(err.is_empty(), "{name}: expected error {err:?}");
                    assert_eq!(got.into(), value, "{name}: value");
                    if check_position {
                        assert_eq!(bs.p, offset + width, "{name}: position");
                    }
                }
                Err(got) => assert_eq!(got.message(), err, "{name}: error"),
            }
        }
    }

    #[test]
    fn read_byte1() {
        let rows: &[Row<'_>] = &[
            (
                "Bit out of bounds",
                &TEST_DATA,
                80,
                0,
                "expected 1 bit at bit 80, but the byte array was only 6 bytes long",
            ),
            ("0 in first byte", &TEST_DATA, 2, 0, ""),
            ("1 in first byte", &TEST_DATA, 5, 1, ""),
            ("1 in last bit, 3rd byte", &TEST_DATA, 23, 1, ""),
            ("Last bit last byte", &TEST_DATA, 47, 1, ""),
        ];
        run_rows(rows, 1, true, BitStream::read_byte1);
    }

    #[test]
    fn parse_byte2() {
        let rows: &[Row<'_>] = &[
            (
                "Bits overrun",
                &TEST_DATA,
                47,
                0,
                "expected 2 bits to start at bit 47, but the byte array was only 6 bytes long",
            ),
            (
                "Bits out of bounds",
                &TEST_DATA,
                80,
                0,
                "expected 2 bits to start at bit 80, but the byte array was only 6 bytes long",
            ),
            ("Spans 2 bytes", &TEST_DATA, 31, 2, ""),
            ("Spans 2 bytes 2", &TEST_DATA, 23, 3, ""),
            ("nibble aligned", &TEST_DATA, 9, 1, ""),
            ("nibble aligned 2", &TEST_DATA, 10, 2, ""),
            ("nibble aligned 3", &TEST_DATA, 44, 2, ""),
            ("No offset", &TEST_DATA, 0, 0, ""),
        ];
        run_rows(rows, 2, false, BitStream::read_byte2);
    }

    #[test]
    fn read_byte4() {
        let rows: &[Row<'_>] = &[
            (
                "Bits overrun",
                &TEST_DATA,
                46,
                0,
                "expected 4 bits to start at bit 46, but the byte array was only 6 bytes long \
                 (needs second byte)",
            ),
            (
                "Bits out of bounds",
                &TEST_DATA,
                80,
                0,
                "expected 4 bits to start at bit 80, but the byte array was only 6 bytes long",
            ),
            ("Spans 2 bytes", &TEST_DATA, 21, 7, ""),
            ("nibble aligned 1", &TEST_DATA, 12, 2, ""),
            ("nibble aligned 2", &TEST_DATA, 44, 11, ""),
            ("Spans 2 bytes 2", &TEST_DATA, 6, 2, ""),
            ("No offset", &[0x10], 0, 1, ""),
            ("nibble aligned 3", &[0x92], 4, 2, ""),
            ("unaligned, 1 byte", &[0x99], 1, 3, ""),
            ("Spans 2 bytes 3", &[0x01, 0xE0], 7, 15, ""),
        ];
        run_rows(rows, 4, true, BitStream::read_byte4);
    }

    #[test]
    fn read_byte6() {
        let rows: &[Row<'_>] = &[
            (
                "Bits overrun",
                &TEST_DATA,
                46,
                0,
                "expected 6 bits to start at bit 46, but the byte array was only 6 bytes long \
                 (needs second byte)",
            ),
            (
                "Bits out of bounds",
                &TEST_DATA,
                80,
                0,
                "expected 6 bits to start at bit 80, but the byte array was only 6 bytes long",
            ),
            ("Spans 2 bytes", &TEST_DATA, 21, 29, ""),
            ("Nibble aligned", &TEST_DATA, 12, 8, ""),
            ("Spans 2 bytes 2", &TEST_DATA, 6, 10, ""),
            ("No offset", &[0x10], 0, 4, ""),
            ("nibble aligned 2", &[0x92, 0x80], 4, 10, ""),
            // Go's unmasked shift: 0x99 >> 1.
            ("unaligned", &[0x99], 1, 76, ""),
            ("spans 2 bytes 3", &[0x01, 0xE0], 7, 60, ""),
        ];
        run_rows(rows, 6, true, BitStream::read_byte6);
    }

    #[test]
    fn read_byte6_go_shift_quirk() {
        // Byte-aligned reads past the first byte shift by a wrapped `2 - bitStartIndex`.
        assert_eq!(stream(&TEST_DATA, 8).read_byte6(), Ok(0), "bit 8");
        assert_eq!(stream(&TEST_DATA, 2).read_byte6(), Ok(0x04), "bit 2");
    }

    #[test]
    fn read_two_bit_field() {
        let rows: &[TwoBitRow<'_>] = &[
            (
                "Read 6 bitfields",
                &TEST_DATA,
                0,
                6,
                &[0, 0, 1, 0, 2, 2],
                "",
            ),
            (
                "Read 8 bitfields",
                &TEST_DATA,
                12,
                8,
                &[0, 2, 0, 0, 0, 3, 2, 3],
                "",
            ),
            (
                "Read 10 bitfields",
                &TEST_DATA,
                28,
                10,
                &[0, 1, 0, 0, 0, 0, 0, 2, 2, 3],
                "",
            ),
            (
                "Error reading end of bitstream",
                &TEST_DATA,
                48,
                2,
                &[],
                "expected 2 bits to start at bit 48, but the byte array was only 6 bytes long",
            ),
            (
                "Error passing in invalid number of fields",
                &TEST_DATA,
                0,
                -2,
                &[],
                "numFields is invalid",
            ),
        ];
        for &(name, data, offset, fields, value, err) in rows {
            match stream(data, offset).read_two_bit_field(fields) {
                Ok(got) => {
                    assert!(err.is_empty(), "{name}: expected error {err:?}");
                    assert_eq!(got, value, "{name}: value");
                }
                Err(got) => assert_eq!(got.message(), err, "{name}: error"),
            }
        }
    }

    #[test]
    fn read_byte8() {
        let rows: &[Row<'_>] = &[
            (
                "Bits overrun",
                &[0x44, 0x76],
                11,
                0,
                "expected 8 bits to start at bit 11, but the byte array was only 2 bytes long",
            ),
            (
                "Bits out of bounds",
                &[0x44, 0x76],
                18,
                0,
                "expected 8 bits to start at bit 18, but the byte array was only 2 bytes long",
            ),
            ("nibble aligned", &TEST_DATA, 4, 0x4A, ""),
            ("odd offset", &TEST_DATA, 7, 81, ""),
            ("even offset", &TEST_DATA, 26, 196, ""),
            ("even offset 2", &TEST_DATA, 6, 40, ""),
            ("zero offset", &TEST_DATA, 8, 162, ""),
        ];
        run_rows(rows, 8, true, BitStream::read_byte8);
    }

    #[test]
    fn read_uint12() {
        let rows: &[Row<'_>] = &[
            (
                "Bytes overrun",
                &TEST_DATA,
                44,
                0,
                "expected a 12-bit int to start at bit 44, but the byte array was only 6 bytes \
                 long",
            ),
            (
                "Bits overrun",
                &TEST_DATA,
                40,
                0,
                "expected a 12-bit int to start at bit 40, but the byte array was only 6 bytes \
                 long",
            ),
            (
                "Bits out of bounds",
                &TEST_DATA,
                80,
                0,
                "expected a 12-bit int to start at bit 80, but the byte array was only 6 bytes \
                 long",
            ),
            ("Even offset, 2 bytes", &TEST_DATA, 10, 2176, ""),
            ("Zero offset", &TEST_DATA, 16, 59, ""),
            ("Odd offset, 3 bytes", &TEST_DATA, 19, 472, ""),
            ("Odd offset, 2 bytes", &TEST_DATA, 1, 148, ""),
            ("Even offset, 3 bytes", &TEST_DATA, 22, 3780, ""),
            ("Nibble aligned", &TEST_DATA, 4, 1186, ""),
            ("Corner case", &TEST_DATA, 36, 0x2B, ""),
        ];
        run_rows(rows, 12, true, BitStream::read_uint12);
    }

    #[test]
    fn read_uint16() {
        let rows: &[Row<'_>] = &[
            (
                "Bytes overrun",
                &TEST_DATA,
                44,
                0,
                "expected a 16-bit int to start at bit 44, but the byte array was only 6 bytes \
                 long",
            ),
            (
                "Bits overrun",
                &TEST_DATA,
                40,
                0,
                "expected a 16-bit int to start at bit 40, but the byte array was only 6 bytes \
                 long",
            ),
            (
                "Bits out of bounds",
                &TEST_DATA,
                80,
                0,
                "expected a 16-bit int to start at bit 80, but the byte array was only 6 bytes \
                 long",
            ),
            ("Even offset", &TEST_DATA, 10, 34830, ""),
            ("Zero offset", &TEST_DATA, 16, 945, ""),
            ("Odd offset", &TEST_DATA, 19, 7560, ""),
            ("Odd offset 2", &TEST_DATA, 1, 2372, ""),
            ("Even offset 2", &TEST_DATA, 22, 60480, ""),
            ("Nibble aligned", &TEST_DATA, 4, 18976, ""),
        ];
        run_rows(rows, 16, true, BitStream::read_uint16);
    }

    #[test]
    fn position_wraps_like_go_uint16() {
        let mut bs = stream(&vec![0xFF; 8192], 65_535);
        assert_eq!(bs.read_byte1(), Ok(1), "last bit");
        assert_eq!(bs.position(), 0, "wrapped position");
    }

    #[test]
    fn length_checks_use_truncated_length() {
        // 65536 + 1 bytes: Go's uint16(len) is 1, so bit 8 is "out of bounds" while the message
        // prints the real length.
        let err = stream(&vec![0; 65_537], 8).read_byte1().err();
        assert_eq!(
            err.as_ref().map(GppError::message),
            Some("expected 1 bit at bit 8, but the byte array was only 65537 bytes long"),
            "truncated length"
        );
    }

    #[test]
    fn from_base64_pads_last_quantum() {
        let decode = |s: &str| BitStream::from_base64(s).map(|bs| bs.b);
        assert_eq!(
            decode("DBABM"),
            Ok(vec![0x0C, 0x10, 0x01, 0x30]),
            "5 chars + A"
        );
        assert_eq!(
            decode("DBABMA"),
            Ok(vec![0x0C, 0x10, 0x01, 0x30, 0]),
            "6 chars + A"
        );
        assert_eq!(
            decode("DBABMAAA"),
            Ok(vec![0x0C, 0x10, 0x01, 0x30, 0, 0]),
            "8 chars"
        );
        assert_eq!(
            decode("DB.A").map_err(|e| e.to_string()),
            Err("illegal base64 data at input byte 2".to_owned()),
            "error"
        );
    }
}
