//! Go: `unicode/utf8` decoding of byte strings that may not be valid UTF-8.
//!
//! Go strings are arbitrary bytes. Where Go decodes one into runes (`for range`, `[]rune(s)`,
//! `encoding/json` output), each byte that does not start a valid UTF-8 sequence becomes one
//! `U+FFFD`. [`lossy_string`] applies that rule; Rust's `String::from_utf8_lossy` instead replaces
//! each maximal invalid subsequence with a single `U+FFFD` (`"\xe2\x82"` gives one replacement
//! character in Rust, two in Go).

/// Converts Go string bytes to a Rust string, replacing every byte that does not start a valid
/// UTF-8 sequence with `U+FFFD`, as Go's `string([]rune(s))` does.
#[must_use]
pub fn lossy_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for _ in chunk.invalid() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_each_invalid_byte() {
        assert_eq!(
            lossy_string(b"a\xe2\x82b"),
            "a\u{FFFD}\u{FFFD}b",
            "truncated sequence"
        );
        assert_eq!(
            lossy_string(b"\xed\xa0\x80"),
            "\u{FFFD}\u{FFFD}\u{FFFD}",
            "surrogate"
        );
        assert_eq!(lossy_string(b"\xc0\xaf"), "\u{FFFD}\u{FFFD}", "overlong");
        assert_eq!(lossy_string("\u{20ac}".as_bytes()), "\u{20ac}", "valid");
    }
}
