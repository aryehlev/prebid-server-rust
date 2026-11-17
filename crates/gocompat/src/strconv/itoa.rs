//! Go: `internal/strconv/itoa.go` (`FormatUint`, `FormatInt`, `Itoa`).

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Go: `strconv.FormatUint(i, base)`: lower-case digits for bases above 10.
///
/// Go panics for a base outside `2..=36`; this returns an empty string instead.
#[must_use]
pub fn format_uint(i: u64, base: u32) -> String {
    format_bits(i, base, false)
}

/// Go: `strconv.FormatInt(i, base)`.
///
/// Go panics for a base outside `2..=36`; this returns an empty string instead.
#[must_use]
pub fn format_int(i: i64, base: u32) -> String {
    format_bits(i.unsigned_abs(), base, i < 0)
}

/// Go: `strconv.Itoa(i)` (`FormatInt(int64(i), 10)`).
#[must_use]
pub fn itoa(i: i64) -> String {
    i.to_string()
}

fn format_bits(mut u: u64, base: u32, neg: bool) -> String {
    if !(2..=36).contains(&base) {
        return String::new();
    }
    let base = u64::from(base);
    let mut out = Vec::with_capacity(65);
    loop {
        let digit = usize::try_from(u % base).unwrap_or_default();
        out.push(DIGITS.get(digit).copied().unwrap_or(b'0'));
        u /= base;
        if u == 0 {
            break;
        }
    }
    if neg {
        out.push(b'-');
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}
