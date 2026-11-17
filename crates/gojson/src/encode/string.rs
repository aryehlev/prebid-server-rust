//! String escaping for each encoder flavour.

use crate::unicode::decode_rune;

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex(nibble: u8) -> u8 {
    HEX.get(usize::from(nibble & 0xF)).copied().unwrap_or(b'0')
}

/// ASCII bytes that need no escaping when HTML escaping is on (`htmlSafeSet` in both libraries).
const fn html_safe(b: u8) -> bool {
    b >= 0x20 && b < 0x80 && b != b'"' && b != b'\\' && b != b'<' && b != b'>' && b != b'&'
}

/// ASCII bytes that need no escaping when HTML escaping is off (`safeSet`).
const fn safe(b: u8) -> bool {
    b >= 0x20 && b < 0x80 && b != b'"' && b != b'\\'
}

/// jsoniter `Stream.WriteStringWithHTMLEscaped`: used for string values and string map keys.
pub(crate) fn jsoniter_html_string(out: &mut Vec<u8>, s: &[u8]) {
    // Go: json-iterator/go stream_str.go WriteStringWithHTMLEscaped /
    // writeStringSlowPathWithHTMLEscaped
    out.push(b'"');
    let mut start = 0;
    let mut i = 0;
    while let Some(&b) = s.get(i) {
        if b < 0x80 {
            if html_safe(b) {
                i += 1;
                continue;
            }
            out.extend_from_slice(s.get(start..i).unwrap_or_default());
            match b {
                b'\\' | b'"' => out.extend_from_slice(&[b'\\', b]),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                b'\t' => out.extend_from_slice(b"\\t"),
                _ => out.extend_from_slice(&[b'\\', b'u', b'0', b'0', hex(b >> 4), hex(b)]),
            }
            i += 1;
            start = i;
            continue;
        }
        let (r, size) = decode_rune(s.get(i..).unwrap_or_default());
        match r {
            None => {
                out.extend_from_slice(s.get(start..i).unwrap_or_default());
                out.extend_from_slice(b"\\ufffd");
                i += 1;
                start = i;
            }
            Some(c @ ('\u{2028}' | '\u{2029}')) => {
                out.extend_from_slice(s.get(start..i).unwrap_or_default());
                out.extend_from_slice(b"\\u202");
                out.push(hex(u8::try_from(u32::from(c) & 0xF).unwrap_or(0)));
                i += size;
                start = i;
            }
            Some(_) => i += size,
        }
    }
    out.extend_from_slice(s.get(start..).unwrap_or_default());
    out.push(b'"');
}

/// jsoniter `Stream.WriteString` (no HTML escaping, no UTF-8 handling): used for struct field
/// names and for the outer layer of `,string` on string fields.
pub(crate) fn jsoniter_plain_string(out: &mut Vec<u8>, s: &[u8]) {
    // Go: json-iterator/go stream_str.go WriteString / writeStringSlowPath
    out.push(b'"');
    let mut start = 0;
    let mut i = 0;
    while let Some(&b) = s.get(i) {
        if b >= 0x80 || safe(b) {
            i += 1;
            continue;
        }
        out.extend_from_slice(s.get(start..i).unwrap_or_default());
        match b {
            b'\\' | b'"' => out.extend_from_slice(&[b'\\', b]),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            _ => out.extend_from_slice(&[b'\\', b'u', b'0', b'0', hex(b >> 4), hex(b)]),
        }
        i += 1;
        start = i;
    }
    out.extend_from_slice(s.get(start..).unwrap_or_default());
    out.push(b'"');
}

/// `encoding/json` `appendString`.
pub(crate) fn std_string(out: &mut Vec<u8>, s: &[u8], escape_html: bool) {
    // Go: encoding/json/encode.go appendString
    out.push(b'"');
    let mut start = 0;
    let mut i = 0;
    while let Some(&b) = s.get(i) {
        if b < 0x80 {
            if html_safe(b) || (!escape_html && safe(b)) {
                i += 1;
                continue;
            }
            out.extend_from_slice(s.get(start..i).unwrap_or_default());
            match b {
                b'\\' | b'"' => out.extend_from_slice(&[b'\\', b]),
                0x08 => out.extend_from_slice(b"\\b"),
                0x0C => out.extend_from_slice(b"\\f"),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                b'\t' => out.extend_from_slice(b"\\t"),
                _ => out.extend_from_slice(&[b'\\', b'u', b'0', b'0', hex(b >> 4), hex(b)]),
            }
            i += 1;
            start = i;
            continue;
        }
        let (r, size) = decode_rune(s.get(i..).unwrap_or_default());
        match r {
            None => {
                out.extend_from_slice(s.get(start..i).unwrap_or_default());
                out.extend_from_slice(b"\\ufffd");
                i += 1;
                start = i;
            }
            Some(c @ ('\u{2028}' | '\u{2029}')) => {
                out.extend_from_slice(s.get(start..i).unwrap_or_default());
                out.extend_from_slice(b"\\u202");
                out.push(hex(u8::try_from(u32::from(c) & 0xF).unwrap_or(0)));
                i += size;
                start = i;
            }
            Some(_) => i += size,
        }
    }
    out.extend_from_slice(s.get(start..).unwrap_or_default());
    out.push(b'"');
}

/// `encoding/json` `appendHTMLEscape`: escapes `<`, `>`, `&`, U+2028 and U+2029 only.
pub(crate) fn html_escape(out: &mut Vec<u8>, src: &[u8]) {
    // Go: encoding/json/indent.go appendHTMLEscape
    let mut start = 0;
    let mut i = 0;
    while let Some(&c) = src.get(i) {
        if c == b'<' || c == b'>' || c == b'&' {
            out.extend_from_slice(src.get(start..i).unwrap_or_default());
            out.extend_from_slice(&[b'\\', b'u', b'0', b'0', hex(c >> 4), hex(c)]);
            start = i + 1;
        }
        if c == 0xE2
            && src.get(i + 1) == Some(&0x80)
            && src.get(i + 2).is_some_and(|&b| b & !1 == 0xA8)
        {
            out.extend_from_slice(src.get(start..i).unwrap_or_default());
            out.extend_from_slice(b"\\u202");
            out.push(hex(src.get(i + 2).copied().unwrap_or(0)));
            start = i + 3;
        }
        i += 1;
    }
    out.extend_from_slice(src.get(start..).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::{jsoniter_html_string, jsoniter_plain_string, std_string};

    fn run(f: fn(&mut Vec<u8>, &[u8]), s: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        f(&mut out, s);
        out
    }

    fn check(got: &[u8], want: &[u8], what: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!(
                "{what}: got {:?}, want {:?}",
                String::from_utf8_lossy(got),
                String::from_utf8_lossy(want)
            ))
        }
    }

    #[test]
    fn escapes_per_flavour() -> Result<(), String> {
        // Go: jsoniter Stream.WriteStringWithHTMLEscaped / WriteString, encoding/json
        // appendString with and without HTML escaping
        let input = "<a>&\u{8}\u{c}\n\u{1}\u{2028}\u{7f}".as_bytes();
        check(
            &run(jsoniter_html_string, input),
            b"\"\\u003ca\\u003e\\u0026\\u0008\\u000c\\n\\u0001\\u2028\x7f\"",
            "jsoniter html",
        )?;
        check(
            &run(|o, s| std_string(o, s, true), input),
            b"\"\\u003ca\\u003e\\u0026\\b\\f\\n\\u0001\\u2028\x7f\"",
            "std html",
        )?;
        check(
            &run(|o, s| std_string(o, s, false), input),
            b"\"<a>&\\b\\f\\n\\u0001\\u2028\x7f\"",
            "std no html",
        )?;
        check(
            &run(jsoniter_plain_string, b"a\"\xff"),
            b"\"a\\\"\xff\"",
            "jsoniter plain keeps invalid bytes",
        )
    }
}
