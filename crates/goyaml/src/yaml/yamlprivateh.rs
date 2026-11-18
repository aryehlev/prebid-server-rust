//! Go: `yamlprivateh.go`. The character classes of the working buffer (`is_alpha`, `is_blank`,
//! ...) are methods of the parser in `yamlh.rs`, reading the buffer at an absolute index.

/// Go: `width`: the length of the UTF-8 sequence a leading octet starts, or 0.
pub(super) const fn width(b: u8) -> usize {
    if b & 0x80 == 0x00 {
        return 1;
    }
    if b & 0xE0 == 0xC0 {
        return 2;
    }
    if b & 0xF0 == 0xE0 {
        return 3;
    }
    if b & 0xF8 == 0xF0 {
        return 4;
    }
    0
}
