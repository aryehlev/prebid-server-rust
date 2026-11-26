//! Reading the fields the Go golden generators write with `strconv.QuoteToASCII` (shared by the
//! test binaries that read them).

use core::str::from_utf8;

/// The bytes of a field `strconv.QuoteToASCII` wrote: Go's `strconv.Unquote`, for the escapes
/// `QuoteToASCII` writes (`\a \b \f \n \r \t \v \\ \"`, `\xNN` for a byte that is not UTF-8,
/// `\uNNNN` and `\UNNNNNNNN`) between double quotes.
///
/// # Errors
///
/// The field, when it is not such a literal.
pub(super) fn unquote(field: &str) -> Result<Vec<u8>, String> {
    let bad = || format!("not a QuoteToASCII literal: {field}");
    let inner = field
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(bad)?;
    let mut out = Vec::with_capacity(inner.len());
    let mut rest = inner.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        rest = tail;
        if b != b'\\' {
            if !(b' '..=b'~').contains(&b) || b == b'"' {
                return Err(bad());
            }
            out.push(b);
            continue;
        }
        let (&escape, after) = rest.split_first().ok_or_else(bad)?;
        rest = after;
        let digits = match escape {
            b'x' => 2,
            b'u' => 4,
            b'U' => 8,
            _ => {
                out.push(match escape {
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'v' => 0x0B,
                    b'\\' | b'"' => escape,
                    _ => return Err(bad()),
                });
                continue;
            }
        };
        let hex = rest.get(..digits).ok_or_else(bad)?;
        rest = rest.get(digits..).ok_or_else(bad)?;
        let value = from_utf8(hex)
            .ok()
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .ok_or_else(bad)?;
        if escape == b'x' {
            out.push(u8::try_from(value).ok().ok_or_else(bad)?);
        } else {
            let c = char::from_u32(value).ok_or_else(bad)?;
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    Ok(out)
}
