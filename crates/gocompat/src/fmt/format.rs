//! Go: `fmt/format.go`, the low-level formatter behind each verb.

use std::iter;

use crate::strconv::{
    can_backquote_bytes, format_float, is_print, quote_bytes, quote_rune, quote_rune_to_ascii,
    quote_to_ascii_bytes,
};

pub(super) const LDIGITS: &[u8; 17] = b"0123456789abcdefx";
pub(super) const UDIGITS: &[u8; 17] = b"0123456789ABCDEFX";

/// One of Go's `fmtFlags` booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Flag {
    Minus = 1,
    Plus = 2,
    Sharp = 4,
    Space = 8,
    Zero = 16,
    /// `%+v`: the plus flag moved aside so that `%+v` is its own flagless format.
    PlusV = 32,
    /// `%#v`: the sharp flag moved aside, as for `PlusV`.
    SharpV = 64,
}

/// Go: `fmtFlags` plus the width and precision of `fmt`. `wid`/`prec` are Go's `wid`/`prec`
/// when `widPresent`/`precPresent` is set.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Flags {
    pub(super) bits: u8,
    pub(super) wid: Option<i64>,
    pub(super) prec: Option<i64>,
}

fn digit(digits: &[u8; 17], i: u64) -> u8 {
    usize::try_from(i)
        .ok()
        .and_then(|i| digits.get(i))
        .copied()
        .unwrap_or(b'?')
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Go: `utf8.RuneCount`: each invalid byte counts as one rune.
fn rune_count(b: &[u8]) -> i64 {
    to_i64(
        b.utf8_chunks()
            .map(|c| c.valid().chars().count() + c.invalid().len())
            .sum(),
    )
}

/// Go: `rune(c)` with values above `utf8.MaxRune` (and surrogates) as U+FFFD.
fn to_rune(c: u64) -> char {
    u32::try_from(c)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
}

impl Flags {
    pub(super) const fn has(self, flag: Flag) -> bool {
        self.bits & (flag as u8) != 0
    }

    pub(super) const fn set(&mut self, flag: Flag, on: bool) {
        if on {
            self.bits |= flag as u8;
        } else {
            self.bits &= !(flag as u8);
        }
    }

    /// Go: `clearflags`.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Go: `writePadding`.
    pub(super) fn write_padding(self, buf: &mut Vec<u8>, n: i64) {
        if n <= 0 {
            return;
        }
        // Zero padding is allowed only to the left.
        let pad = if self.has(Flag::Zero) && !self.has(Flag::Minus) {
            b'0'
        } else {
            b' '
        };
        buf.extend(iter::repeat_n(pad, usize::try_from(n).unwrap_or(0)));
    }

    /// Go: `pad`: appends `b`, padded to the width on the left (or right with `-`).
    pub(super) fn pad(self, buf: &mut Vec<u8>, b: &[u8]) {
        let Some(wid) = self.wid.filter(|&w| w != 0) else {
            buf.extend_from_slice(b);
            return;
        };
        let width = wid - rune_count(b);
        if self.has(Flag::Minus) {
            buf.extend_from_slice(b);
            self.write_padding(buf, width);
        } else {
            self.write_padding(buf, width);
            buf.extend_from_slice(b);
        }
    }

    /// Go: `padString`.
    pub(super) fn pad_string(self, buf: &mut Vec<u8>, s: &str) {
        self.pad(buf, s.as_bytes());
    }

    /// Go: `pad` with the zero flag cleared.
    fn pad_no_zero(mut self, buf: &mut Vec<u8>, b: &[u8]) {
        self.set(Flag::Zero, false);
        self.pad(buf, b);
    }

    /// Go: `fmtBoolean`.
    pub(super) fn fmt_boolean(self, buf: &mut Vec<u8>, v: bool) {
        self.pad_string(buf, if v { "true" } else { "false" });
    }

    /// Go: `fmtUnicode`: `U+0078`, or `U+0078 'x'` with `#`.
    pub(super) fn fmt_unicode(self, buf: &mut Vec<u8>, u: u64) {
        let prec = self.prec.filter(|&p| p > 4).unwrap_or(4);
        let hex = format!("{u:X}");
        let zeros = usize::try_from(prec).unwrap_or(0).saturating_sub(hex.len());
        let mut out = format!("U+{}{hex}", "0".repeat(zeros));
        if self.has(Flag::Sharp)
            && let Some(c) = u32::try_from(u).ok().and_then(char::from_u32)
            && is_print(c)
        {
            out.push_str(" '");
            out.push(c);
            out.push('\'');
        }
        self.pad_no_zero(buf, out.as_bytes());
    }

    /// Go: `fmtInteger`.
    pub(super) fn fmt_integer(
        self,
        buf: &mut Vec<u8>,
        u: u64,
        base: u64,
        is_signed: bool,
        verb: char,
        digits: &[u8; 17],
    ) {
        let negative = is_signed && u.cast_signed() < 0;
        let mut u = if negative { u.wrapping_neg() } else { u };
        // Two ways to ask for extra leading zero digits: %.3d or %03d. If both are specified
        // the zero flag is ignored and padding with spaces is used instead.
        let mut prec = 0;
        if let Some(p) = self.prec {
            prec = p;
            // Precision of 0 and value of 0 means "print nothing" but padding.
            if prec == 0 && u == 0 {
                let mut no_zero = self;
                no_zero.set(Flag::Zero, false);
                no_zero.write_padding(buf, self.wid.unwrap_or(0));
                return;
            }
        } else if let Some(wid) = self.wid
            && self.has(Flag::Zero)
            && !self.has(Flag::Minus)
        {
            prec = wid;
            if negative || self.has(Flag::Plus) || self.has(Flag::Space) {
                prec -= 1; // leave room for sign
            }
        }
        // Digits, least significant first.
        let mut rev = Vec::new();
        while u >= base {
            rev.push(digit(digits, u % base));
            u /= base;
        }
        rev.push(digit(digits, u));
        while to_i64(rev.len()) < prec {
            rev.push(b'0');
        }
        if self.has(Flag::Sharp) {
            match base {
                2 => rev.extend_from_slice(b"b0"),
                8 if rev.last() != Some(&b'0') => rev.push(b'0'),
                16 => rev.extend_from_slice(&[digit(digits, 16), b'0']),
                _ => {}
            }
        }
        if verb == 'O' {
            rev.extend_from_slice(b"o0");
        }
        if negative {
            rev.push(b'-');
        } else if self.has(Flag::Plus) {
            rev.push(b'+');
        } else if self.has(Flag::Space) {
            rev.push(b' ');
        }
        rev.reverse();
        // Left padding with zeros has already been handled like precision earlier or the zero
        // flag is ignored due to an explicitly set precision.
        self.pad_no_zero(buf, &rev);
    }

    /// Go: `truncate`/`truncateString`: at most `prec` runes, as `utf8.DecodeRune` counts them.
    fn truncate(self, b: &[u8]) -> &[u8] {
        let Some(prec) = self.prec else {
            return b;
        };
        let mut n = prec;
        let mut i = 0;
        for chunk in b.utf8_chunks() {
            let widths = chunk
                .valid()
                .chars()
                .map(char::len_utf8)
                .chain(chunk.invalid().iter().map(|_| 1));
            for width in widths {
                if n <= 0 {
                    return b.get(..i).unwrap_or(b);
                }
                n -= 1;
                i += width;
            }
        }
        b
    }

    /// Go: `fmtS` and `fmtBs`.
    pub(super) fn fmt_s(self, buf: &mut Vec<u8>, s: &[u8]) {
        self.pad(buf, self.truncate(s));
    }

    /// Go: `fmtSbx`: the hexadecimal encoding of the bytes of a string or byte slice.
    pub(super) fn fmt_sbx(self, buf: &mut Vec<u8>, b: &[u8], digits: &[u8; 17]) {
        let mut length = to_i64(b.len());
        // Set length to not process more bytes than the precision demands.
        if let Some(prec) = self.prec
            && prec < length
        {
            length = prec;
        }
        let (sharp, space) = (self.has(Flag::Sharp), self.has(Flag::Space));
        // Compute width of the encoding taking into account the sharp and space flags.
        let mut width = 2 * length;
        if width > 0 {
            if space {
                // Each element encoded by two hexadecimals will get a leading 0x or 0X.
                if sharp {
                    width *= 2;
                }
                // Elements will be separated by a space.
                width += length - 1;
            } else if sharp {
                // Only a leading 0x or 0X will be added for the whole string.
                width += 2;
            }
        } else {
            // The byte slice or string that should be encoded is empty.
            if let Some(wid) = self.wid {
                self.write_padding(buf, wid);
            }
            return;
        }
        let pad = self.wid.filter(|&w| w > width).map_or(0, |w| w - width);
        // Handle padding to the left.
        if !self.has(Flag::Minus) {
            self.write_padding(buf, pad);
        }
        let prefix = [b'0', digit(digits, 16)];
        if sharp {
            buf.extend_from_slice(&prefix);
        }
        for (i, &c) in b
            .iter()
            .take(usize::try_from(length).unwrap_or(0))
            .enumerate()
        {
            if space && i > 0 {
                buf.push(b' ');
                if sharp {
                    buf.extend_from_slice(&prefix);
                }
            }
            buf.push(digit(digits, u64::from(c >> 4)));
            buf.push(digit(digits, u64::from(c & 0xF)));
        }
        // Handle padding to the right.
        if self.has(Flag::Minus) {
            self.write_padding(buf, pad);
        }
    }

    /// Go: `fmtQ`: a quoted Go string, or a raw string with `#` when possible.
    pub(super) fn fmt_q(self, buf: &mut Vec<u8>, s: &[u8]) {
        let s = self.truncate(s);
        if self.has(Flag::Sharp) && can_backquote_bytes(s) {
            let mut raw = Vec::with_capacity(s.len() + 2);
            raw.push(b'`');
            raw.extend_from_slice(s);
            raw.push(b'`');
            self.pad(buf, &raw);
            return;
        }
        let q = if self.has(Flag::Plus) {
            quote_to_ascii_bytes(s)
        } else {
            quote_bytes(s)
        };
        self.pad_string(buf, &q);
    }

    /// Go: `fmtC`: the character, or U+FFFD when not valid.
    pub(super) fn fmt_c(self, buf: &mut Vec<u8>, c: u64) {
        self.pad_string(buf, to_rune(c).encode_utf8(&mut [0; 4]));
    }

    /// Go: `fmtQc`: a quoted Go character literal.
    pub(super) fn fmt_qc(self, buf: &mut Vec<u8>, c: u64) {
        let r = to_rune(c);
        let q = if self.has(Flag::Plus) {
            quote_rune_to_ascii(r)
        } else {
            quote_rune(r)
        };
        self.pad_string(buf, &q);
    }

    /// Go: `fmtFloat`. `verb` is a `strconv.FormatFloat` format byte.
    pub(super) fn fmt_float(self, buf: &mut Vec<u8>, v: f64, size: i64, verb: u8, prec: i64) {
        // Explicit precision in format specifier overrules default precision.
        let prec = self.prec.unwrap_or(prec);
        let (plus, space) = (self.has(Flag::Plus), self.has(Flag::Space));
        // Format number, reserving space for leading + sign if needed.
        let formatted = format_float(v, verb, prec, size);
        let mut num = Vec::with_capacity(formatted.len() + 1);
        if !formatted.starts_with(['-', '+']) {
            num.push(b'+');
        }
        num.extend_from_slice(formatted.as_bytes());
        // The space flag means a leading space instead of "+" unless plus is also set.
        if space
            && !plus
            && let Some(sign) = num.first_mut()
            && *sign == b'+'
        {
            *sign = b' ';
        }
        // Infinities and NaN don't look like a number so shouldn't be padded with zeros.
        if matches!(num.get(1), Some(b'I' | b'N')) {
            // Remove sign before NaN if not asked for.
            let skip = usize::from(num.get(1) == Some(&b'N') && !space && !plus);
            self.pad_no_zero(buf, num.get(skip..).unwrap_or_default());
            return;
        }
        // The sharp flag forces printing a decimal point for non-binary formats and retains
        // trailing zeros, which we may need to restore.
        if self.has(Flag::Sharp) && verb != b'b' {
            sharp_float(&mut num, verb, prec);
        }
        let unsigned = num.get(1..).unwrap_or_default();
        // We want a sign if asked for and if the sign is not positive.
        if plus || num.first() != Some(&b'+') {
            // If we're zero padding to the left we want the sign before the leading zeros.
            let len = to_i64(num.len());
            if self.has(Flag::Zero)
                && !self.has(Flag::Minus)
                && let Some(wid) = self.wid.filter(|&w| w > len)
            {
                buf.extend(num.first());
                self.write_padding(buf, wid - len);
                buf.extend_from_slice(unsigned);
                return;
            }
            self.pad(buf, &num);
            return;
        }
        // No sign to show and the number is positive; just print the unsigned number.
        self.pad(buf, unsigned);
    }
}

/// The `#` handling of Go's `fmtFloat`: a decimal point and the trailing zeros up to the
/// precision (`%#g` keeps 6 significant digits by default).
fn sharp_float(num: &mut Vec<u8>, verb: u8, prec: i64) {
    let mut digits = match verb {
        // If no precision is set explicitly use a precision of 6.
        b'v' | b'g' | b'G' | b'x' if prec == -1 => 6,
        b'v' | b'g' | b'G' | b'x' => prec,
        _ => 0,
    };
    let mut tail = Vec::new();
    let mut has_decimal_point = false;
    let mut saw_nonzero_digit = false;
    // Starting from i = 1 to skip the sign at num[0].
    let mut i = 1;
    while let Some(&c) = num.get(i) {
        match c {
            b'.' => has_decimal_point = true,
            b'p' | b'P' => {
                tail.extend(num.drain(i..));
                break;
            }
            b'e' | b'E' if verb != b'x' && verb != b'X' => {
                tail.extend(num.drain(i..));
                break;
            }
            _ => {
                if c != b'0' {
                    saw_nonzero_digit = true;
                }
                // Count significant digits after the first non-zero digit.
                if saw_nonzero_digit {
                    digits -= 1;
                }
            }
        }
        i += 1;
    }
    if !has_decimal_point {
        // Leading digit 0 should contribute once to digits.
        if num.len() == 2 && num.get(1) == Some(&b'0') {
            digits -= 1;
        }
        num.push(b'.');
    }
    while digits > 0 {
        num.push(b'0');
        digits -= 1;
    }
    num.extend(tail);
}
