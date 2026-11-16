//! Go `strconv`: number formatting and parsing with Go's exact output and error texts.
//!
//! Go: `strconv/number.go` (the exported wrappers and `NumError`) over `internal/strconv`
//! (`atoi.go`, `atof.go`, `atob.go`, `itoa.go`, `ftoa.go`) and `strconv/quote.go`. Go 1.26.
//!
//! - Parsing returns [`NumError`], whose `Display` is Go's `Error()`: `strconv.ParseInt: parsing
//!   "x": invalid syntax`.
//! - Go `int` is 64 bits on every platform Prebid runs on, so `bit_size == 0` means 64 and [`atoi`]
//!   returns `i64`.
//! - [`format_float`] produces Go's digits for every format byte (`e E f g G b x X`): shortest
//!   round-trip digits for a negative precision, correctly rounded (ties to even) digits otherwise.
//! - Go functions that panic on invalid arguments (an illegal base for [`format_int`], an illegal
//!   bit size for [`format_float`]) return an empty string here instead.

mod atob;
mod atof;
mod atoi;
mod ftoa;
mod itoa;
mod quote;

use std::{error::Error as StdError, fmt};

pub use atob::{format_bool, parse_bool};
pub use atof::{parse_float, parse_float_pair};
pub use atoi::{atoi, atoi_pair, parse_int, parse_int_pair, parse_uint, parse_uint_pair};
pub use ftoa::format_float;
pub use itoa::{format_int, format_uint, itoa};
pub use quote::{
    UnquoteError, can_backquote, can_backquote_bytes, is_graphic, is_print, quote, quote_bytes,
    quote_rune, quote_rune_to_ascii, quote_rune_to_graphic, quote_to_ascii, quote_to_ascii_bytes,
    quote_to_graphic, unquote, unquote_bytes, unquote_char,
};

/// Go: `strconv.IntSize`.
pub const INT_SIZE: i64 = 64;

/// The reason a conversion failed (Go: `NumError.Err`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumErrorKind {
    /// Go: `strconv.ErrRange` (`value out of range`).
    Range,
    /// Go: `strconv.ErrSyntax` (`invalid syntax`).
    Syntax,
    /// Go: `errors.New("invalid base " + Itoa(base))`.
    Base(i64),
    /// Go: `errors.New("invalid bit size " + Itoa(bitSize))`.
    BitSize(i64),
}

impl fmt::Display for NumErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Range => f.write_str("value out of range"),
            Self::Syntax => f.write_str("invalid syntax"),
            Self::Base(base) => write!(f, "invalid base {base}"),
            Self::BitSize(bits) => write!(f, "invalid bit size {bits}"),
        }
    }
}

/// Go: `*strconv.NumError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumError {
    /// Go: `NumError.Func`, the failing function (`ParseInt`, `ParseFloat`, `Atoi`, ...).
    pub func: &'static str,
    /// Go: `NumError.Num`, the input.
    pub num: String,
    /// Go: `NumError.Err`.
    pub err: NumErrorKind,
}

impl NumError {
    fn new(func: &'static str, num: &str, err: NumErrorKind) -> Self {
        Self {
            func,
            num: num.to_owned(),
            err,
        }
    }
}

impl fmt::Display for NumError {
    /// Go: `"strconv." + e.Func + ": " + "parsing " + Quote(e.Num) + ": " + e.Err.Error()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "strconv.{}: parsing {}: {}",
            self.func,
            quote(&self.num),
            self.err
        )
    }
}

impl StdError for NumError {}

/// Go: `internal/strconv.lower`.
const fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

/// Go: `internal/strconv.underscoreOK`: underscores may only separate digits (or follow a base
/// prefix).
fn underscore_ok(s: &[u8]) -> bool {
    let s = match s.first() {
        Some(b'+' | b'-') => s.get(1..).unwrap_or_default(),
        _ => s,
    };
    // saw is one of '^' (start), '0' (digit or base prefix), '_' or '!' (other).
    let (mut saw, hex, rest) = match s {
        [b'0', prefix, tail @ ..] if matches!(lower(*prefix), b'b' | b'o' | b'x') => {
            // base prefix counts as a digit for "underscore as digit separator"
            (b'0', lower(*prefix) == b'x', tail)
        }
        _ => (b'^', false, s),
    };
    for &c in rest {
        if c.is_ascii_digit() || (hex && lower(c).is_ascii_hexdigit()) {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}

#[cfg(test)]
mod tests;
