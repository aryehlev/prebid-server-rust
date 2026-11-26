//! Glue (ruling 10 (b), D-FASTGZIP): which requests get gzip, read from Accept-Encoding as
//! gziphandler reads it, so pbs-server compresses for the requests Go compresses for
//! (`server.gzip-negotiation`).
//!
//! `acceptsGzip` (gziphandler v1.1.1 gzip.go:450-453) reads the request's first Accept-Encoding
//! value with `parseEncodings` and `parseCoding` (gzip.go:483-532): the value is split at commas
//! and each item at semicolons, and each part is trimmed of Unicode spaces. An item's first part,
//! lower-cased, is its coding, and its last part sets its q: the number after `q=`, clamped to 0
//! to 1, or 1 when the last part is no `q=`. An item whose last `q=` does not parse, or whose
//! coding is empty, is dropped; a later item of a coding replaces an earlier one. The request
//! gets gzip when gzip's q is above 0.
//!
//! The number after `q=` is read by std's float parser where Go has `strconv.ParseFloat` (Rule A:
//! no client sends a q the two read otherwise). Go also reads hexadecimal mantissas (`0x1p-1`)
//! and `_` between digits (`1_0`), which std refuses, so the item is dropped; Go refuses a sign
//! before `nan` and a number beyond the largest float (`1e400`), which std reads as NaN and as
//! infinity. Those q values are `server.gzip-qvalue-syntax`.

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
            // `server.gzip-qvalue-syntax`: std's syntax where Go's is `strconv.ParseFloat`'s.
            match number.parse::<f64>() {
                // Go: NaN is kept, as neither bound holds for it.
                Ok(value) => {
                    q = value.clamp(0.0, 1.0);
                    error = false;
                }
                Err(_) => error = true,
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
