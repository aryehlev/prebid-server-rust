//! Go's `encoding/base64.StdEncoding`, used for `[]byte` values.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn symbol(index: u32) -> u8 {
    usize::try_from(index & 0x3F)
        .ok()
        .and_then(|i| ALPHABET.get(i).copied())
        .unwrap_or(b'A')
}

/// Appends the padded standard base64 encoding of `src`.
pub(crate) fn encode_std(src: &[u8], out: &mut Vec<u8>) {
    for chunk in src.chunks(3) {
        let b0 = u32::from(chunk.first().copied().unwrap_or(0));
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let v = (b0 << 16) | (b1 << 8) | b2;
        out.push(symbol(v >> 18));
        out.push(symbol(v >> 12));
        out.push(if chunk.len() > 1 {
            symbol(v >> 6)
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 { symbol(v) } else { b'=' });
    }
}

fn decode_symbol(c: u8) -> Option<u32> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(v))
}

fn corrupt(offset: usize) -> String {
    format!("illegal base64 data at input byte {offset}")
}

/// Go's `base64.StdEncoding.Decode`: the decoded bytes, or the bytes decoded before the error
/// together with the `CorruptInputError` text.
pub(crate) fn decode_std(src: &[u8]) -> Result<Vec<u8>, (Vec<u8>, String)> {
    // Go: encoding/base64/base64.go Decode / decodeQuantum (non-strict, '=' padding)
    let mut out = Vec::with_capacity(src.len() / 4 * 3);
    let mut si = 0;
    while si < src.len() {
        let mut dbuf = [0_u32; 4];
        let mut dlen = 4;
        let mut j = 0;
        let mut pending_err: Option<String> = None;
        while j < 4 {
            let Some(&input) = src.get(si) else {
                if j == 0 {
                    return Ok(out);
                }
                // A partial quantum without padding is corrupt (StdEncoding pads).
                return Err((out, corrupt(si - j)));
            };
            si += 1;
            if let Some(v) = decode_symbol(input) {
                if let Some(slot) = dbuf.get_mut(j) {
                    *slot = v;
                }
                j += 1;
                continue;
            }
            if input == b'\n' || input == b'\r' {
                continue;
            }
            if input != b'=' {
                return Err((out, corrupt(si - 1)));
            }
            // Padding.
            match j {
                0 | 1 => return Err((out, corrupt(si - 1))),
                2 => {
                    while matches!(src.get(si), Some(b'\n' | b'\r')) {
                        si += 1;
                    }
                    if si == src.len() {
                        return Err((out, corrupt(src.len())));
                    }
                    if src.get(si) != Some(&b'=') {
                        return Err((out, corrupt(si - 1)));
                    }
                    si += 1;
                }
                _ => {}
            }
            while matches!(src.get(si), Some(b'\n' | b'\r')) {
                si += 1;
            }
            if si < src.len() {
                pending_err = Some(corrupt(si));
            }
            dlen = j;
            break;
        }
        let val = (dbuf[0] << 18) | (dbuf[1] << 12) | (dbuf[2] << 6) | dbuf[3];
        let bytes = val.to_be_bytes();
        let produced = dlen.saturating_sub(1);
        out.extend_from_slice(bytes.get(1..1 + produced).unwrap_or_default());
        if let Some(err) = pending_err {
            return Err((out, err));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{decode_std, encode_std};

    /// `Ok` when the values are equal, otherwise an error naming the case.
    macro_rules! check {
        ($got:expr, $want:expr, $what:expr $(,)?) => {{
            let (got, want) = ($got, $want);
            if got == want {
                Ok(())
            } else {
                Err(format!("{}: got {got:?}, want {want:?}", $what))
            }
        }};
    }

    #[test]
    fn round_trips() -> Result<(), String> {
        // Expected values from Go's base64.StdEncoding (Encode/Decode with partial output).
        let mut out = Vec::new();
        encode_std(b"hello!?", &mut out);
        check!(out.as_slice(), b"aGVsbG8hPw==", "encode")?;
        check!(decode_std(&out), Ok(b"hello!?".to_vec()), "decode")?;
        check!(
            decode_std(b"aGV$"),
            Err((Vec::new(), "illegal base64 data at input byte 3".to_owned())),
            "corrupt",
        )?;
        check!(
            decode_std(b"aGVsbA"),
            Err((
                b"hel".to_vec(),
                "illegal base64 data at input byte 4".to_owned(),
            )),
            "missing padding",
        )
    }
}
