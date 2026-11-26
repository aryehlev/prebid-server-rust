//! Glue (D-FASTGZIP, the user's ruling 10 (b)): which requests get gzip, read from
//! Accept-Encoding as gziphandler reads it, so pbs-server compresses for the requests Go
//! compresses for (`server.gzip-negotiation`).
//!
//! `acceptsGzip` (gziphandler v1.1.1 gzip.go:450-453) reads the request's first Accept-Encoding
//! value with `parseEncodings` and `parseCoding` (gzip.go:483-532): the value is split at commas
//! and each item at semicolons, and each part is trimmed of Unicode spaces. An item's first part,
//! lower-cased, is its coding, and its last part sets its q: the number after `q=`, read by
//! `strconv.ParseFloat` and clamped to 0 to 1, or 1 when the last part is no `q=`. An item whose
//! last `q=` does not parse, or whose coding is empty, is dropped; a later item of a coding
//! replaces an earlier one. The request gets gzip when gzip's q is above 0.
//!
//! The q values follow `strconv.ParseFloat`'s syntax, as pbs-config's loader ports it
//! (crates/config/src/loader/goconv.rs): signs, `_` between digits, hexadecimal mantissas with a
//! `p` exponent, `inf`, `infinity` and `nan`, an exponent that stops growing past 10,000, and an
//! error for a number beyond the largest float. A decimal number is rebuilt from the digits and
//! the exponent Go reads and parsed by std's correctly rounded parser, as Go's parser rounds
//! correctly; a hexadecimal one is rounded as Go's `atofHex` rounds it.

/// gziphandler's `DefaultQValue`: the q of a coding whose last part is no `q=`.
const DEFAULT_Q: f64 = 1.0;

/// gziphandler's `acceptsGzip`: whether a request whose first Accept-Encoding value is `value`
/// (`None` when it has none) gets a gzip answer.
// Go: acceptsGzip (gziphandler v1.1.1 gzip.go:450-453)
pub(super) fn accepts_gzip(value: Option<&[u8]>) -> bool {
    gzip_q(value).is_some_and(|q| q > 0.0)
}

/// gziphandler's `parseEncodings(value)["gzip"]`: gzip's q, clamped to 0 to 1, or `None` where
/// the value gives gzip none.
// Go: parseEncodings (gziphandler v1.1.1 gzip.go:483-504)
pub(super) fn gzip_q(value: Option<&[u8]>) -> Option<f64> {
    // A Go string holds any bytes. One that is not UTF-8 is no space, digit or letter of a
    // coding Go matches, so U+FFFD in its place reads alike.
    let value = String::from_utf8_lossy(value.unwrap_or_default());
    // Go: a later item of a coding replaces an earlier one.
    value
        .split(',')
        .rev()
        .filter_map(parse_coding)
        .find(|(coding, _)| is_gzip(coding))
        .map(|(_, q)| q)
}

/// gziphandler's `parseCoding` of one item: its coding, trimmed but not lower-cased, and its q;
/// `None` where Go returns an error.
// Go: parseCoding (gziphandler v1.1.1 gzip.go:509-532)
fn parse_coding(item: &str) -> Option<(&str, f64)> {
    let mut parts = item.split(';');
    let coding = parts.next().unwrap_or_default().trim();
    let mut q = DEFAULT_Q;
    let mut error = false;
    for part in parts {
        // Go: each part sets q afresh, so the last one decides; the error of a `q=` that does
        // not parse stays until another `q=` parses.
        q = DEFAULT_Q;
        if let Some(number) = part.trim().strip_prefix("q=") {
            match parse_float(number) {
                // Go: NaN is kept, as neither bound holds for it.
                Some(value) => {
                    q = value.clamp(0.0, 1.0);
                    error = false;
                }
                None => error = true,
            }
        }
    }
    (!error && !coding.is_empty()).then_some((coding, q))
}

/// Whether `coding`, lower-cased as `strings.ToLower` lower-cases it (`unicode.ToLower` on each
/// rune), is `gzip`. Besides ASCII's capitals only U+0130 lower-cases to one of its letters.
fn is_gzip(coding: &str) -> bool {
    let mut chars = coding.chars();
    matches!(
        [
            chars.next(),
            chars.next(),
            chars.next(),
            chars.next(),
            chars.next()
        ],
        [
            Some('g' | 'G'),
            Some('z' | 'Z'),
            Some('i' | 'I' | '\u{130}'),
            Some('p' | 'P'),
            None
        ]
    )
}

/// An ASCII letter in lower case; other bytes never compare equal to one after it.
const fn lower(c: u8) -> u8 {
    c | 0x20
}

/// Go: `strconv.ParseFloat(text, 64)` where it returns no error, and `None` where it returns
/// one, a syntax error or a range error.
// Go: ParseFloat and atof64 (internal/strconv/atof.go:614-698)
pub(super) fn parse_float(text: &str) -> Option<f64> {
    let s = text.as_bytes();
    if let Some((value, n)) = special(s) {
        return (n == s.len()).then_some(value);
    }
    let read = read_float(s)?;
    if read.end != s.len() {
        return None;
    }
    let value = if read.hex {
        hex_float(&read)
    } else {
        decimal_float(&read)?
    };
    // Go: a finite number beyond the largest float is a range error.
    (!value.is_infinite()).then_some(value)
}

/// Go: `strconv`'s `special`: `inf` or `infinity` after an optional sign, or `nan` without one,
/// in any case, at the start of `s`: the value and how many bytes it takes.
// Go: special (internal/strconv/atof.go:37-67)
fn special(s: &[u8]) -> Option<(f64, usize)> {
    let (negative, nsign, rest) = match s.first() {
        Some(b'+') => (false, 1, s.get(1..).unwrap_or_default()),
        Some(b'-') => (true, 1, s.get(1..).unwrap_or_default()),
        _ => (false, 0, s),
    };
    let common = |word: &[u8]| {
        rest.iter()
            .zip(word)
            .take_while(|(a, b)| lower(**a) == **b)
            .count()
    };
    match rest.first() {
        Some(b'i' | b'I') => {
            let mut n = common(b"infinity");
            // Go: anything longer than "inf" is fine, but short of "infinity" only "inf" counts.
            if 3 < n && n < 8 {
                n = 3;
            }
            let value = if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            (n == 3 || n == 8).then_some((value, nsign + n))
        }
        // Go: a sign before "nan" is not accepted.
        Some(b'n' | b'N') if nsign == 0 => (common(b"nan") == 3).then_some((f64::NAN, 3)),
        _ => None,
    }
}

/// What Go's `readFloat` read.
#[derive(Debug)]
struct Read {
    /// A leading `-`.
    negative: bool,
    /// A `0x` mantissa.
    hex: bool,
    /// The bytes read.
    end: usize,
    /// Decimal: the digits from the first non-zero one, and the power of ten they are scaled by.
    digits: String,
    /// See `digits`.
    exp10: i64,
    /// Hexadecimal: the first 16 significant digits.
    mantissa: u64,
    /// Hexadecimal: whether a non-zero digit followed those 16.
    trunc: bool,
    /// Hexadecimal: the power of two the mantissa is scaled by.
    exp2: i64,
}

/// The digits part of Go's `readFloat` loop.
#[derive(Debug, Default)]
struct Digits {
    /// An `_` among the digits.
    underscores: bool,
    /// Go: `sawdot`.
    sawdot: bool,
    /// Go: `sawdigits`.
    sawdigits: bool,
    /// Go: `nd`.
    nd: i64,
    /// Go: `ndMant`.
    nd_mant: i64,
    /// Go: `dp`.
    dp: i64,
    /// Go: `mantissa`.
    mantissa: u64,
    /// Go: `trunc`.
    trunc: bool,
    /// The decimal digits from the first non-zero one.
    text: String,
}

impl Digits {
    /// Reads digits, `_` and one `.` of `s` from `i` (hexadecimal digits with `hex`), and returns
    /// where they end.
    // Go: readFloat's digits loop (internal/strconv/atof.go:203-246)
    fn read(&mut self, s: &[u8], mut i: usize, hex: bool) -> usize {
        let (base, max_mant_digits) = if hex { (16, 16) } else { (10, 19) };
        while let Some(&c) = s.get(i) {
            let value = if c.is_ascii_digit() {
                c - b'0'
            } else if hex && (b'a'..=b'f').contains(&lower(c)) {
                lower(c) - b'a' + 10
            } else if c == b'_' {
                self.underscores = true;
                i += 1;
                continue;
            } else if c == b'.' && !self.sawdot {
                self.sawdot = true;
                self.dp = self.nd;
                i += 1;
                continue;
            } else {
                return i;
            };
            self.sawdigits = true;
            i += 1;
            // Go: leading zeros are skipped.
            if value == 0 && self.nd == 0 {
                self.dp -= 1;
                continue;
            }
            self.nd += 1;
            if !hex {
                self.text.push(char::from(c));
            }
            if self.nd_mant < max_mant_digits {
                self.mantissa = self.mantissa * base + u64::from(value);
                self.nd_mant += 1;
            } else if value != 0 {
                self.trunc = true;
            }
        }
        i
    }
}

/// The exponent part of Go's `readFloat` at `s[i..]`, after its `e` or `p`: an optional sign,
/// then digits and `_`. Returns the exponent, whether it held an `_`, and where it ends; `None`
/// where it has no digit.
// Go: readFloat's exponent (internal/strconv/atof.go:264-290)
fn read_exponent(s: &[u8], mut i: usize) -> Option<(i64, bool, usize)> {
    let mut esign = 1;
    match s.get(i)? {
        b'+' => i += 1,
        b'-' => {
            i += 1;
            esign = -1;
        }
        _ => {}
    }
    if !s.get(i).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut e = 0_i64;
    let mut underscores = false;
    while let Some(&c) = s.get(i) {
        if c == b'_' {
            underscores = true;
        } else if c.is_ascii_digit() {
            // Go: a very large exponent only needs to be large.
            if e < 10_000 {
                e = e * 10 + i64::from(c - b'0');
            }
        } else {
            break;
        }
        i += 1;
    }
    Some((e * esign, underscores, i))
}

/// Go: `strconv`'s `readFloat`: a sign, decimal digits with an optional point and `e` exponent
/// or `0x` and hexadecimal digits with a mandatory `p` exponent, and `_` between digits. `None`
/// where Go's `ok` is false.
// Go: readFloat (internal/strconv/atof.go:171-306)
fn read_float(s: &[u8]) -> Option<Read> {
    let (negative, mut i) = match s.first()? {
        b'+' => (false, 1),
        b'-' => (true, 1),
        _ => (false, 0),
    };
    let hex =
        i + 2 < s.len() && s.get(i) == Some(&b'0') && s.get(i + 1).map(|c| lower(*c)) == Some(b'x');
    if hex {
        i += 2;
    }
    let mut d = Digits::default();
    i = d.read(s, i, hex);
    if !d.sawdigits {
        return None;
    }
    let mut dp = if d.sawdot { d.dp } else { d.nd };
    let mut nd_mant = d.nd_mant;
    if hex {
        dp *= 4;
        nd_mant *= 4;
    }
    let mut underscores = d.underscores;
    let exp_char = if hex { b'p' } else { b'e' };
    if s.get(i).map(|c| lower(*c)) == Some(exp_char) {
        let (e, separated, end) = read_exponent(s, i + 1)?;
        dp += e;
        underscores |= separated;
        i = end;
    } else if hex {
        return None;
    }
    if underscores && !underscore_ok(s.get(..i).unwrap_or_default()) {
        return None;
    }
    Some(Read {
        negative,
        hex,
        end: i,
        exp10: if hex { 0 } else { dp - d.nd },
        digits: d.text,
        mantissa: d.mantissa,
        trunc: d.trunc,
        exp2: if d.mantissa == 0 { 0 } else { dp - nd_mant },
    })
}

/// Go: `strconv`'s `underscoreOK`: an `_` may only separate digits, or follow a base prefix.
// Go: underscoreOK (internal/strconv/atoi.go:251-296)
fn underscore_ok(s: &[u8]) -> bool {
    let s = match s.first() {
        Some(b'+' | b'-') => s.get(1..).unwrap_or_default(),
        _ => s,
    };
    // saw is '^' (start), '0' (a digit or the base prefix), '_' or '!' (anything else).
    let (mut saw, hex, rest) = match s {
        [b'0', prefix, tail @ ..] if matches!(lower(*prefix), b'b' | b'o' | b'x') => {
            (b'0', lower(*prefix) == b'x', tail)
        }
        _ => (b'^', false, s),
    };
    for &c in rest {
        if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(c))) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else if saw == b'_' {
            return false;
        } else {
            saw = b'!';
        }
    }
    saw != b'_'
}

/// A decimal [`Read`] as the nearest float, by std's parser.
fn decimal_float(read: &Read) -> Option<f64> {
    let sign = if read.negative { "-" } else { "" };
    let text = if read.digits.is_empty() {
        format!("{sign}0")
    } else {
        format!("{sign}{}e{}", read.digits, read.exp10)
    };
    text.parse().ok()
}

/// Go: `strconv`'s `atofHex` for 64 bits: a hexadecimal [`Read`] rounded to the nearest float,
/// ties to even, and infinite where it overflows.
// Go: atofHex (internal/strconv/atof.go:496-559) with float64info
fn hex_float(read: &Read) -> f64 {
    const FRACTION_WIDTH: u32 = 52;
    const EXPONENT_BIAS: i64 = -1023;
    const TOP_EXP: i64 = (1 << 11) + EXPONENT_BIAS - 2;
    const BOTTOM_EXP: i64 = EXPONENT_BIAS + 1;
    let mut mantissa = read.mantissa;
    // The mantissa is now implicitly divided by 2^52.
    let mut exp = read.exp2 + i64::from(FRACTION_WIDTH);
    // A leading 1 and 52 more bits, then two rounding bits, the last of them sticky.
    while mantissa != 0 && mantissa >> (FRACTION_WIDTH + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if read.trunc {
        mantissa |= 1;
    }
    while mantissa >> (FRACTION_WIDTH + 3) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Denormalize an exponent below the range (the -2 is for the rounding bits).
    while mantissa > 1 && exp < BOTTOM_EXP - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (FRACTION_WIDTH + 1) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> FRACTION_WIDTH == 0 {
        exp = EXPONENT_BIAS;
    }
    if exp > TOP_EXP {
        return if read.negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let biased = u64::try_from((exp - EXPONENT_BIAS) & 0x7FF).unwrap_or(0);
    let mut bits = (mantissa & ((1 << FRACTION_WIDTH) - 1)) | (biased << FRACTION_WIDTH);
    if read.negative {
        bits |= 1 << 63;
    }
    f64::from_bits(bits)
}
