//! Port of go-gdpr `bitutils/bitutils.go`: reads big-endian bit fields out of a decoded consent
//! string, with Go's exact bounds checks and error messages.
//!
//! Bits are numbered from the most significant bit of `data[0]`. Every function fails with
//! [`ConsentError::Malformed`] holding Go's message when `data` is too short.

use crate::ConsentError;

/// Returns `data[index]`.
///
/// Every caller has already checked the length with Go's own condition; reading through `get`
/// keeps the port free of panicking indexing.
fn byte(data: &[u8], index: usize) -> u8 {
    data.get(index).copied().unwrap_or_default()
}

/// Go: `bitutils.ParseByte4`, the 4 bits starting at `bit_start_index`.
///
/// # Errors
///
/// Fails when the bits lie past the end of `data`.
pub fn parse_byte4(data: &[u8], bit_start_index: usize) -> Result<u8, ConsentError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    if bit_start_offset < 5 {
        if data.len() < start_byte + 1 {
            return Err(ConsentError::Malformed(format!(
                "ParseByte4 expected 4 bits to start at bit {bit_start_index}, but the consent \
                 string was only {} bytes long",
                data.len()
            )));
        }
        return Ok((byte(data, start_byte) & (0xF0 >> bit_start_offset)) >> (4 - bit_start_offset));
    }
    if data.len() < start_byte + 2 {
        return Err(ConsentError::Malformed(format!(
            "ParseByte4 expected 4 bits to start at bit {bit_start_index}, but the consent string \
             was only {} bytes long (needs second byte)",
            data.len()
        )));
    }

    let left_bits = (byte(data, start_byte) & (0xF0 >> bit_start_offset)) << (bit_start_offset - 4);
    let bits_consumed = 8 - bit_start_offset;
    let overflow = 4 - bits_consumed;
    let right_bits = (byte(data, start_byte + 1) & (0xF0 << (4 - overflow))) >> (8 - overflow);
    Ok(left_bits | right_bits)
}

/// Go: `bitutils.ParseByte8`, the 8 bits starting at `bit_start_index`.
///
/// # Errors
///
/// Fails when the bits lie past the end of `data`.
pub fn parse_byte8(data: &[u8], bit_start_index: usize) -> Result<u8, ConsentError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    if bit_start_offset == 0 {
        if data.len() < start_byte + 1 {
            return Err(ConsentError::Malformed(format!(
                "ParseByte8 expected 8 bits to start at bit {bit_start_index}, but the consent \
                 string was only {} bytes long",
                data.len()
            )));
        }
        return Ok(byte(data, start_byte));
    }
    if data.len() < start_byte + 2 {
        // Go: bitutils/bitutils.go:40 spells "bitst".
        return Err(ConsentError::Malformed(format!(
            "ParseByte8 expected 8 bitst to start at bit {bit_start_index}, but the consent string \
             was only {} bytes long",
            data.len()
        )));
    }

    let left_bits = (byte(data, start_byte) & (0xFF >> bit_start_offset)) << bit_start_offset;
    let shift_complement = 8 - bit_start_offset;
    let right_bits = (byte(data, start_byte + 1) & (0xFF << shift_complement)) >> shift_complement;
    Ok(left_bits | right_bits)
}

/// Go: `bitutils.ParseUInt12`, the 12-bit big-endian integer starting at `bit_start_index`.
///
/// # Errors
///
/// Fails when the bits lie past the end of `data`.
pub fn parse_uint12(data: &[u8], bit_start_index: usize) -> Result<u16, ConsentError> {
    let end = bit_start_index + 12;
    let mut end_byte = end / 8;
    if !end.is_multiple_of(8) {
        end_byte += 1;
    }
    if data.len() < end_byte {
        return Err(ConsentError::Malformed(format!(
            "ParseUInt12 expected a 12-bit int to start at bit {bit_start_index}, but the consent \
             string was only {} bytes long",
            data.len()
        )));
    }

    let left_byte = parse_byte4(data, bit_start_index)
        .map_err(|err| ConsentError::Malformed(format!("ParseUInt12 error on left byte: {err}")))?;
    let right_byte = parse_byte8(data, bit_start_index + 4).map_err(|err| {
        ConsentError::Malformed(format!("ParseUInt12 error on right byte: {err}"))
    })?;
    Ok(u16::from_be_bytes([left_byte, right_byte]))
}

/// Go: `bitutils.ParseUInt16`, the 16-bit big-endian integer starting at `bit_start_index`.
///
/// # Errors
///
/// Fails when the bits lie past the end of `data`.
pub fn parse_uint16(data: &[u8], bit_start_index: usize) -> Result<u16, ConsentError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    let required = if bit_start_offset == 0 {
        start_byte + 2
    } else {
        start_byte + 3
    };
    if data.len() < required {
        return Err(ConsentError::Malformed(format!(
            "ParseUInt16 expected a 16-bit int to start at bit {bit_start_index}, but the consent \
             string was only {} bytes long",
            data.len()
        )));
    }
    if bit_start_offset == 0 {
        return Ok(u16::from_be_bytes([
            byte(data, start_byte),
            byte(data, start_byte + 1),
        ]));
    }

    let left_byte = parse_byte8(data, bit_start_index)
        .map_err(|err| ConsentError::Malformed(format!("ParseUInt16 error on left byte: {err}")))?;
    let right_byte = parse_byte8(data, bit_start_index + 8).map_err(|err| {
        ConsentError::Malformed(format!("ParseUInt16 error on right byte: {err}"))
    })?;
    Ok(u16::from_be_bytes([left_byte, right_byte]))
}

/// Go: `bitutils/bitutils_test.go`.
#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{parse_byte4, parse_byte8, parse_uint12, parse_uint16};
    use crate::{
        ConsentError,
        test_support::{TestResult, hex_decode, load_cases, str_field},
    };

    const TESTDATA: &[u8] = &[0x04, 0xA2, 0x03, 0xB1, 0x00, 0x2B];

    fn message<T>(result: Result<T, ConsentError>) -> String {
        result.err().map_or_else(String::new, |err| err.to_string())
    }

    /// Renders a result the way the Go generator records it: `v:<n>` or `e:<error text>`.
    fn render<T: Into<u64>>(result: Result<T, ConsentError>) -> String {
        match result {
            Ok(value) => format!("v:{}", value.into()),
            Err(err) => format!("e:{err}"),
        }
    }

    /// Replays `testdata/bitutils_goldens.json`: every start bit from 0 to 16 bits past the end
    /// of 54 buffers, for all four functions.
    #[test]
    fn go_goldens() -> TestResult {
        let cases = load_cases("bitutils_goldens.json")?;
        check_eq!(cases.len(), 54, "case count");
        for case in &cases {
            let data = hex_decode(str_field(case, "data_hex").ok_or("missing data_hex")?)?;
            let column = |key: &str| -> Vec<String> {
                case.get(key)
                    .and_then(Value::as_array)
                    .map_or_else(Vec::new, |values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
            };
            let (byte4, byte8, uint12, uint16) = (
                column("byte4"),
                column("byte8"),
                column("uint12"),
                column("uint16"),
            );
            check_eq!(byte4.len(), data.len() * 8 + 17, "start bits for {data:?}");
            for start in 0..byte4.len() {
                let expect = |values: &[String]| values.get(start).cloned().unwrap_or_default();
                check_eq!(
                    render(parse_byte4(&data, start)),
                    expect(&byte4),
                    "byte4 {start}"
                );
                check_eq!(
                    render(parse_byte8(&data, start)),
                    expect(&byte8),
                    "byte8 {start}"
                );
                check_eq!(
                    render(parse_uint12(&data, start)),
                    expect(&uint12),
                    "uint12 {start}"
                );
                check_eq!(
                    render(parse_uint16(&data, start)),
                    expect(&uint16),
                    "uint16 {start}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn test_parse_byte4() -> TestResult {
        check_eq!(
            message(parse_byte4(TESTDATA, 46)),
            "ParseByte4 expected 4 bits to start at bit 46, but the consent string was only 6 \
             bytes long (needs second byte)",
            "bit 46"
        );
        check_eq!(
            message(parse_byte4(TESTDATA, 80)),
            "ParseByte4 expected 4 bits to start at bit 80, but the consent string was only 6 \
             bytes long",
            "bit 80"
        );
        let cases: [(&[u8], usize, u8); 8] = [
            (TESTDATA, 21, 7),
            (TESTDATA, 12, 2),
            (TESTDATA, 44, 11),
            (TESTDATA, 6, 2),
            (&[0x10], 0, 1),
            (&[0x92], 4, 2),
            (&[0x99], 1, 3),
            (&[0x01, 0xE0], 7, 15),
        ];
        for (data, offset, value) in cases {
            check_eq!(parse_byte4(data, offset)?, value, "{data:?} at {offset}");
        }
        Ok(())
    }

    #[test]
    fn test_parse_byte8() -> TestResult {
        check_eq!(
            message(parse_byte8(&[0x44, 0x76], 11)),
            "ParseByte8 expected 8 bitst to start at bit 11, but the consent string was only 2 \
             bytes long",
            "bit 11"
        );
        check_eq!(
            message(parse_byte8(&[0x44, 0x76], 18)),
            "ParseByte8 expected 8 bitst to start at bit 18, but the consent string was only 2 \
             bytes long",
            "bit 18"
        );
        for (offset, value) in [(4, 0x4A), (7, 81), (26, 196), (6, 40), (8, 162)] {
            check_eq!(parse_byte8(TESTDATA, offset)?, value, "offset {offset}");
        }
        Ok(())
    }

    #[test]
    fn test_parse_uint12() -> TestResult {
        for offset in [44, 40, 37] {
            check_eq!(
                message(parse_uint12(TESTDATA, offset)),
                format!(
                    "ParseUInt12 expected a 12-bit int to start at bit {offset}, but the consent \
                     string was only 6 bytes long"
                ),
                "offset {offset}"
            );
        }
        for (offset, value) in [
            (10, 2176),
            (16, 59),
            (19, 472),
            (1, 148),
            (22, 3780),
            (4, 1186),
            (36, 0x2B),
        ] {
            check_eq!(parse_uint12(TESTDATA, offset)?, value, "offset {offset}");
        }
        Ok(())
    }

    #[test]
    fn test_parse_uint16() -> TestResult {
        for offset in [44, 40] {
            check_eq!(
                message(parse_uint16(TESTDATA, offset)),
                format!(
                    "ParseUInt16 expected a 16-bit int to start at bit {offset}, but the consent \
                     string was only 6 bytes long"
                ),
                "offset {offset}"
            );
        }
        for (offset, value) in [
            (10, 34830),
            (16, 945),
            (19, 7560),
            (1, 2372),
            (22, 60480),
            (4, 18976),
        ] {
            check_eq!(parse_uint16(TESTDATA, offset)?, value, "offset {offset}");
        }
        Ok(())
    }
}
