//! Fibonacci-coded integers and ranges. Go: `util/fibonacci.go`.

use super::{
    bitstream::{BitStream, len16},
    rangesection::{IRange, IntRange},
};
use crate::error::GppError;

/// Number of preloaded Fibonacci values. Go: `fibLen`.
pub(super) const FIB_LEN: usize = 20;

/// Preloaded Fibonacci values. Go: `fibLookup`.
const FIB_LOOKUP: [u16; FIB_LEN] = [
    0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233, 377, 610, 987, 1597, 2584, 4181,
];

/// `fibLookup[i]` for `i < FIB_LEN`.
pub(super) fn fib_at(i: usize) -> u16 {
    FIB_LOOKUP.get(i).copied().unwrap_or_default()
}

/// The largest number of Fibonacci-coded bits Go can ever read in one integer before it either
/// finds the `11` terminator or fails: past this the `uint16` bit position has visited every
/// position of the stream, so Go's read loop would never end.
const MAX_FIBONACCI_LOOP: usize = 65_536 + 3;

/// Go: `BitStream.ReadFibonacciInt`. Note that 0 cannot be Fibonacci encoded.
///
/// Go computes `fibonacci(i)` recursively (exponential time) in `uint16`; this port keeps a
/// running pair of the same wrapping values, so results are identical.
pub(super) fn read_fibonacci_int(bs: &mut BitStream) -> Result<u16, GppError> {
    let mut last_bit = bs.read_byte1().map_err(|err| {
        GppError::new(format!("error reading bit 1 of Integer(Fibonacci): {err}"))
    })?;
    let mut next_bit = bs.read_byte1().map_err(|err| {
        GppError::new(format!("error reading bit 2 of Integer(Fibonacci): {err}"))
    })?;

    // First bit encodes "1" if set. (fib(2)=1)
    let mut result = u16::from(last_bit);

    // fib(i - 1) and fib(i) for the current loop index i, starting at i = 3.
    let (mut fib_prev, mut fib_i) = (fib_at(2), fib_at(3));
    let mut i = 3usize;
    while last_bit == 0 || next_bit == 0 {
        if i > MAX_FIBONACCI_LOOP {
            // Rust-only: Go never leaves this loop (see MAX_FIBONACCI_LOOP); it cannot happen
            // unless the stream holds at least 8192 bytes.
            return Err(GppError::new(format!(
                "error reading bit {i} of Integer(Fibonacci): no terminator in a {} byte stream",
                len16(&bs.b)
            )));
        }
        last_bit = next_bit;
        next_bit = bs.read_byte1().map_err(|err| {
            GppError::new(format!(
                "error reading bit {i} of Integer(Fibonacci): {err}"
            ))
        })?;
        if last_bit == 1 {
            result = result.wrapping_add(fib_i);
        }
        (fib_prev, fib_i) = (fib_i, fib_prev.wrapping_add(fib_i));
        i += 1;
    }

    Ok(result)
}

/// Go: `BitStream.ReadFibonacciRange`.
pub(super) fn read_fibonacci_range(bs: &mut BitStream) -> Result<IntRange, GppError> {
    let num_entries = bs
        .read_uint12()
        .map_err(|err| GppError::new(format!("error reading size of Range(Fibonacci): {err}")))?;
    let mut max_value = 0u16;

    let mut ranges = Vec::with_capacity(usize::from(num_entries));
    for i in 0..num_entries {
        let bit = bs.read_byte1().map_err(|err| {
            GppError::new(format!(
                "error reading the boolean bit of a Range(Fibonacci) entry({i}): {err}"
            ))
        })?;
        if bit == 0 {
            let offset = read_fibonacci_int(bs).map_err(|err| {
                GppError::new(format!(
                    "error reading an int offset value in a Range(Fibonacci) entry({i}): {err}"
                ))
            })?;
            let entry = offset.wrapping_add(max_value);
            ranges.push(IRange {
                start_id: entry,
                end_id: entry,
            });
            if entry > max_value {
                max_value = entry;
            }
        } else {
            // First entry is an offset from the previous entry.
            let offset = read_fibonacci_int(bs).map_err(|err| {
                GppError::new(format!(
                    "error reading first int offset value in a Range(Fibonacci) entry({i}): {err}"
                ))
            })?;
            let start_id = max_value.wrapping_add(offset);
            // Second entry in a Fibonacci range is an offset from the first.
            let length = read_fibonacci_int(bs).map_err(|err| {
                GppError::new(format!(
                    "error reading second int offset value in a Range(Fibonacci) entry({i}): \
                     {err}"
                ))
            })?;
            let end_id = start_id.wrapping_add(length);
            ranges.push(IRange { start_id, end_id });
            if end_id > max_value {
                max_value = end_id;
            }
        }
    }

    Ok(IntRange {
        size: num_entries,
        range: ranges,
        max: max_value,
    })
}

#[cfg(test)]
mod tests {
    //! Go: `util/fibonacci_test.go`.

    use crate::{
        error::GppError,
        util::{BitStream, IRange, IntRange},
    };

    const TEST_DATA: [u8; 6] = [0x04, 0xA2, 0x03, 0xB1, 0x00, 0x2B];

    fn read_at(data: &[u8], p: u16) -> Result<u16, GppError> {
        BitStream {
            p,
            b: data.to_vec(),
        }
        .read_fibonacci_int()
    }

    #[test]
    fn read_fibonacci_int() {
        assert_eq!(
            read_at(&TEST_DATA, 47).map_err(|e| e.to_string()),
            Err(
                "error reading bit 2 of Integer(Fibonacci): expected 1 bit at bit 48, but the \
                 byte array was only 6 bytes long"
                    .to_owned()
            ),
            "overrun"
        );

        let cases: &[(&[u8], u16, u16)] = &[
            (&[0x9B], 6, 1),
            (&[0x2E, 0x2E], 11, 2),
            (&[0xB0], 0, 4),
            (&[0x29, 0x62], 6, 7),
            (&[0x9D, 0x6C], 5, 12),
            (&[0x24, 0x87], 8, 14),
            (&[0x58, 0x40, 0x8C], 5, 1838),
            (&[0x08, 0x11, 0x1A], 0, 12784),
        ];
        for &(data, offset, value) in cases {
            assert_eq!(read_at(data, offset), Ok(value), "{data:?} at {offset}");
        }
    }

    #[test]
    fn read_fibonacci_range() {
        let mut bs = BitStream::new(vec![
            0x00, 0x52, 0xF1, 0xCE, 0xF2, 0x07, 0x24, 0x8A, 0xA3, 0x43,
        ]);
        let expected = IntRange {
            size: 5,
            max: 57257,
            range: vec![
                IRange {
                    start_id: 7,
                    end_id: 7,
                },
                IRange {
                    start_id: 16,
                    end_id: 22,
                },
                IRange {
                    start_id: 24,
                    end_id: 25,
                },
                IRange {
                    start_id: 82,
                    end_id: 82,
                },
                IRange {
                    start_id: 57234,
                    end_id: 57257,
                },
            ],
        };
        assert_eq!(bs.read_fibonacci_range(), Ok(expected), "range");
    }

    #[test]
    fn large_fibonacci_values_wrap_like_go_uint16() {
        // 24 zero bits then "11": bit 24 is worth fib(26) = 121393, which is 55857 in uint16.
        assert_eq!(
            read_at(&[0x00, 0x00, 0x00, 0xC0], 0),
            Ok(55857),
            "fib(26) mod 2^16"
        );
    }

    #[test]
    fn unterminated_integer_in_huge_stream_fails_instead_of_looping() {
        // Go would spin forever: reads never fail (>= 8192 bytes) and no "11" exists.
        let mut bs = BitStream::new(vec![0; 8192]);
        let err = bs.read_fibonacci_int().err();
        assert_eq!(
            err.as_ref().map(GppError::message),
            Some(
                "error reading bit 65540 of Integer(Fibonacci): no terminator in a 8192 byte stream"
            ),
            "Rust-only bail-out"
        );
    }
}
