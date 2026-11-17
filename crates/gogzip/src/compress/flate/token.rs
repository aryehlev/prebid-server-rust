//! Go: `compress/flate/token.go`, the LZ77 tokens the compressors produce.

/// Go: `lengthShift`.
const LENGTH_SHIFT: u32 = 22;
/// Go: `offsetMask`.
const OFFSET_MASK: u32 = (1 << LENGTH_SHIFT) - 1;
/// Go: `literalType`.
const LITERAL_TYPE: u32 = 0;
/// Go: `matchType`.
const MATCH_TYPE: u32 = 1 << 30;

/// Go: `lengthCodes`: the length code for match length `X` is `LENGTH_CODES[X - 3]`.
static LENGTH_CODES: [u8; 256] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 12, 12, 13, 13, 13, 13, 14, 14, 14,
    14, 15, 15, 15, 15, 16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 17, 17, 17, 17, 18, 18, 18,
    18, 18, 18, 18, 18, 19, 19, 19, 19, 19, 19, 19, 19, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
    20, 20, 20, 20, 20, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 22, 22, 22,
    22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23,
    23, 23, 23, 23, 23, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25,
    25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 26, 26, 26,
    26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26,
    26, 26, 26, 26, 26, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27,
    27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 28,
];

/// Go: `offsetCodes`.
static OFFSET_CODES: [u8; 256] = [
    0, 1, 2, 3, 4, 4, 5, 5, 6, 6, 6, 6, 7, 7, 7, 7, 8, 8, 8, 8, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 9,
    10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 11, 11, 11, 11,
    11, 11, 11, 11, 11, 11, 11, 11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14,
    14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14,
    14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 15, 15, 15, 15, 15, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 15,
];

/// Go: `token`: 2 bits of type (0 literal, 1 match), then 8 bits of `length - 3` and 22 bits of
/// `offset - 1` for a match, or the literal value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Token(pub(super) u32);

impl Token {
    /// Go: `literalToken`.
    pub(super) const fn literal_token(literal: u32) -> Self {
        Self(LITERAL_TYPE.wrapping_add(literal))
    }

    /// Go: `matchToken(xlength, xoffset)`: a match of `xlength + 3` bytes at distance
    /// `xoffset + 1`.
    pub(super) const fn match_token(xlength: u32, xoffset: u32) -> Self {
        Self(
            MATCH_TYPE
                .wrapping_add(xlength.wrapping_shl(LENGTH_SHIFT))
                .wrapping_add(xoffset),
        )
    }

    /// Go: `t < matchType`: the token is a literal (or the end-of-block marker).
    pub(super) const fn is_literal(self) -> bool {
        self.0 < MATCH_TYPE
    }

    /// Go: `token.literal`.
    pub(super) const fn literal(self) -> u32 {
        self.0.wrapping_sub(LITERAL_TYPE)
    }

    /// Go: `token.offset`, the extra offset (`offset - 1`) of a match.
    pub(super) const fn offset(self) -> u32 {
        self.0 & OFFSET_MASK
    }

    /// Go: `token.length`, the extra length (`length - 3`) of a match.
    pub(super) const fn length(self) -> u32 {
        self.0.wrapping_sub(MATCH_TYPE) >> LENGTH_SHIFT
    }
}

/// Go: `lengthCode(len)`, for `len` the extra length of a match (0 to 255).
pub(super) fn length_code(len: u32) -> u32 {
    LENGTH_CODES
        .get((len & 0xFF) as usize)
        .map_or(0, |&c| u32::from(c))
}

/// Go: `offsetCode(off)`, for `off` the extra offset of a match.
pub(super) fn offset_code(off: u32) -> u32 {
    let code = |i: u32| OFFSET_CODES.get(i as usize).map_or(0, |&c| u32::from(c));
    if off < 256 {
        return code(off);
    }
    if off >> 7 < 256 {
        return code(off >> 7) + 14;
    }
    code(off >> 14) + 28
}

#[cfg(test)]
mod tests {
    use super::{Token, length_code, offset_code};

    #[test]
    fn token_fields() -> Result<(), String> {
        let t = Token::match_token(255, 0);
        if t.0 != 0x7FC0_0000 || t.is_literal() || t.length() != 255 || t.offset() != 0 {
            return Err(format!("max length token {:#x}", t.0));
        }
        let lit = Token::literal_token(256);
        if !lit.is_literal() || lit.literal() != 256 {
            return Err(format!("end of block token {:#x}", lit.0));
        }
        // RFC 1951 3.2.5: length 258 is code 285 (28 + 257); distance 32768 is code 29.
        if length_code(255) != 28 || offset_code(32767) != 29 || offset_code(255) != 15 {
            return Err("code tables".to_owned());
        }
        Ok(())
    }
}
